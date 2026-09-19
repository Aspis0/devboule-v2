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
            // is made valid here, in this same transaction. Every count is
            // bounded: rows past the read bound, rows whose event does not
            // deserialize, and rows that *are* rewritten all leave the loop
            // one at a time.
            let (rewritten, unreadable, oversized) = backfill_permission_origins(&tx)?;
            if rewritten > 0 || unreadable > 0 || oversized > 0 {
                // Counts only: no payload, no path, no prompt text.
                eprintln!(
                    "journal v9 migration gave {rewritten} stored permission payloads a local \
                     origin, left {unreadable} unreadable payloads alone, and skipped \
                     {oversized} payloads past the {} byte read bound",
                    crate::journal::MAX_ORIGIN_BACKFILL_PAYLOAD_BYTES
                );
            }
        }
        if version < 10 {
            // The display name and the parent of an agent-created session
            // (audit S5-12). NULL is the honest default for both: every row
            // that predates the concept has no name a human chose and no
            // creator, and a surface that finds NULL falls back to the title
            // exactly as it did before these columns existed.
            if !session_has_column(&tx, "display_name")? {
                tx.execute("ALTER TABLE sessions ADD COLUMN display_name TEXT", [])?;
            }
            if !session_has_column(&tx, "created_by")? {
                tx.execute("ALTER TABLE sessions ADD COLUMN created_by TEXT", [])?;
            }
        }
        if version < 11 {
            // What a creation from a profile leaves on its child
            // (`create-from-profile`). `unattended` is the one column with a
            // default, and the reason is the same one the v10 pair gives in
            // reverse: a row that predates this migration was created from no
            // profile, so `0` is an observed fact about it rather than a guess,
            // and a row later written by something that omits the column reads
            // the same way. `profile_id` and `context_id` are nullable, and NULL
            // means what it says — no profile made this session — while a NULL
            // `context_id` reads back as the session's own id, which is the rule
            // the field states.
            if !session_has_column(&tx, "profile_id")? {
                tx.execute("ALTER TABLE sessions ADD COLUMN profile_id TEXT", [])?;
            }
            if !session_has_column(&tx, "context_id")? {
                tx.execute("ALTER TABLE sessions ADD COLUMN context_id TEXT", [])?;
            }
            if !session_has_column(&tx, "unattended")? {
                tx.execute(
                    "ALTER TABLE sessions ADD COLUMN unattended INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
            }
            if !session_has_column(&tx, "labels")? {
                tx.execute("ALTER TABLE sessions ADD COLUMN labels TEXT", [])?;
            }
        }
        if version < 12 {
            // The `unattended` marker becomes three-valued (R2b). The boolean
            // column stays exactly as v11 left it — same shape, same `MAX`
            // ratchet — because a written row's meaning must not be rewritten
            // by an upgrade; the tri-state travels in its own column, encoded
            // as the never-downward order `no=0 < unknown=1 < yes=2` (the
            // encoding `journal.rs::unattended_state_rank` writes and reads).
            //
            // `DEFAULT 1` is `unknown`, and it is the whole backfill rule for
            // the old `false`: an old `0` was derived either by the deleted
            // provider table or by the profile's feature tick, and neither is
            // re-trustable as "a human is watching" — that is what `no` (0)
            // would claim, and **no old row is ever backfilled to it**. Only
            // an old `1` becomes `yes` (2): that row recorded a profile the
            // daemon could see had auto-accepted, which is exactly what `yes`
            // still means.
            if !session_has_column(&tx, "unattended_state")? {
                tx.execute(
                    "ALTER TABLE sessions ADD COLUMN unattended_state INTEGER NOT NULL DEFAULT 1",
                    [],
                )?;
            }
            let promoted = tx.execute(
                "UPDATE sessions SET unattended_state = ?1
                 WHERE unattended = 1 AND unattended_state <> ?1",
                [super::unattended_state_rank(
                    devboule_protocol::UnattendedState::Yes,
                )],
            )?;
            if promoted > 0 {
                // Counts only: no id, no profile, no provider.
                eprintln!(
                    "journal v12 migration marked {promoted} unattended rows as `yes`; \
                     every other existing row reads `unknown`"
                );
            }
            // Every shape the post-commit validation judges is judged **before**
            // the stamp (audit R2b-1 finding 2): all five columns exist by now —
            // just added, or pre-existing with a name that collides — and if any
            // shape is not the one this daemon writes, the open must leave the
            // file at `user_version` 11. Stamping and committing first would
            // brick it: this build refuses the shape at the post-commit
            // validation, and every older build refuses the version at
            // [`JournalError::FutureSchema`], so no build could ever open the
            // file again. The call is the post-commit validation itself —
            // [`validate_profile_columns`] — not a second list kept in step with
            // it, so the two cannot drift: a file the daemon will refuse is a
            // file whose `user_version` never moved, because nothing here has
            // committed and the whole transaction — the `ALTER`s, the promotion,
            // the stamp — rolls back.
            validate_profile_columns(&tx)?;
        }
        if version < 13 {
            // The tool overlay a creation stamps on its child, and the
            // child's own depth. NULL reads as no overlay — the same bytes
            // every pre-v13 row already has — so there is no backfill that
            // could manufacture a restriction nobody recorded, and no
            // default that would claim the opposite. Depth is NULL for the
            // same reason. Both fail-closed readings live in the resume
            // mapping, and only for rows that still name a creator: a row
            // whose `created_by` is gone resumes as an ordinary session
            // either way.
            if !session_has_column(&tx, "overlay")? {
                tx.execute("ALTER TABLE sessions ADD COLUMN overlay TEXT", [])?;
            }
            if !session_has_column(&tx, "depth")? {
                tx.execute("ALTER TABLE sessions ADD COLUMN depth INTEGER", [])?;
            }
            // The pre-stamp guard, same ordering as v12 (audit R2b-1 finding
            // 2): a colliding shape must leave the file at 12, openable by
            // the previous build, rather than stamped 13 and openable by
            // none.
            validate_v13_columns(&tx)?;
        }
        if version < 14 {
            // The disown mark (`crates/devboule-daemon/src/session.rs`, the
            // failed-resume arm). The handle a provider refused is recorded
            // BESIDE `peer_session_id`, never in place of it: the evidence
            // for a refusal is approximate, and the handle is the only route
            // back to the conversation. NULL — every row that predates the
            // column, and every row no provider has refused — is the honest
            // "nothing happened"; a value is a fact learned from the
            // provider, cleared by the next announce of a different handle.
            if !session_has_column(&tx, "disowned_peer_session_id")? {
                tx.execute(
                    "ALTER TABLE sessions ADD COLUMN disowned_peer_session_id TEXT",
                    [],
                )?;
            }
            // The pre-stamp guard, same ordering as v12 and v13: a colliding
            // shape must leave the file at 13, openable by the previous
            // build, rather than stamped 14 — where the first list or replay
            // would die reading the column on a user's journal.
            validate_v14_columns(&tx)?;
        }
        tx.pragma_update(None, "user_version", JOURNAL_SCHEMA_VERSION)?;
        tx.commit()?;
    }
    validate_v6_schema(&conn)?;
    // The two slice-5 columns are checked by shape rather than by presence
    // (audit S5B-07) — see [`validate_agent_columns`].
    validate_agent_columns(&conn)?;
    // The same rule for the five the v11/v12 migrations add — see
    // [`validate_profile_columns`]..
    validate_profile_columns(&conn)?;
    // The v13 columns, checked apart (see [`is_our_overlay_shape`]).
    validate_v13_columns(&conn)?;
    // The v14 column, the same way (see [`is_our_disowned_shape`]).
    validate_v14_columns(&conn)?;
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

/// The two columns the slice-5 migration adds must have the *shape* the daemon
/// writes into them, not merely be present (audit S5B-07).
///
/// `TEXT`, nullable, no default. A v9 database that happens to carry a
/// `display_name INTEGER NOT NULL DEFAULT 'x'` passes a presence check and then
/// answers every read with a value this daemon never wrote; the mismatch takes
/// the corrupt-journal path (`JournalError::Corrupt`, the same one the v6 and
/// v9 shape checks use) rather than being used.
fn validate_agent_columns(conn: &Connection) -> Result<(), JournalError> {
    for column in ["display_name", "created_by"] {
        let shape = column_shape(conn, column)?;
        let ours = matches!(
            shape,
            Some((ref kind, 0, None)) if kind.eq_ignore_ascii_case("text")
        );
        if !ours {
            return Err(JournalError::Corrupt(format!(
                "journal schema has an unexpected sessions.{column} column"
            )));
        }
    }
    Ok(())
}

/// The columns the v11 and v12 migrations add, by shape for the same reason.
///
/// `profile_id`, `context_id` and `labels` are `TEXT`, nullable, no default —
/// the exact shape the daemon writes. `unattended` is the odd one and is checked
/// as `INTEGER NOT NULL DEFAULT 0`: it is a three-state column only if one lies,
/// and this daemon writes `0`/`1` into a column that can never be NULL, so a
/// hand-made `unattended TEXT` (or a nullable one) is a schema this daemon
/// cannot read honestly and takes the corrupt-journal path. Beside it,
/// `unattended_state` is the three-state column — checked as
/// `INTEGER NOT NULL DEFAULT 1`, the rank of `unknown`, because a row written
/// by something that omits the tri-state must read as "not established", never
/// as `no`.
fn validate_profile_columns(conn: &Connection) -> Result<(), JournalError> {
    for column in ["profile_id", "context_id", "labels"] {
        let shape = column_shape(conn, column)?;
        let ours = matches!(
            shape,
            Some((ref kind, 0, None)) if kind.eq_ignore_ascii_case("text")
        );
        if !ours {
            return Err(JournalError::Corrupt(format!(
                "journal schema has an unexpected sessions.{column} column"
            )));
        }
    }
    let shape = column_shape(conn, "unattended")?;
    let ours = matches!(
        shape,
        Some((ref kind, 1, Some(ref default)))
            if kind.eq_ignore_ascii_case("integer") && default == "0"
    );
    if !ours {
        return Err(JournalError::Corrupt(
            "journal schema has an unexpected sessions.unattended column".to_string(),
        ));
    }
    // The v12 tri-state column, by shape for the same reason: `INTEGER NOT
    // NULL DEFAULT 1` — `1` is the rank of `unknown`, so a row written by
    // something that omits the column reads as "not established", never as
    // `no`. A hand-made `unattended_state TEXT` (or a nullable one, or one
    // defaulting to `no`) is a schema this daemon cannot read honestly and
    // takes the corrupt-journal path.
    if !is_our_unattended_state_shape(column_shape(conn, "unattended_state")?) {
        return Err(JournalError::Corrupt(
            "journal schema has an unexpected sessions.unattended_state column".to_string(),
        ));
    }
    Ok(())
}

/// The one shape `unattended_state` may have: `INTEGER NOT NULL DEFAULT 1`,
/// the rank of `unknown` — the shape the v12 migration adds. Both the
/// pre-stamp guard in [`open_connection`] (which calls
/// [`validate_profile_columns`] inside the transaction, before the stamp)
/// and the post-commit [`validate_profile_columns`] read this predicate, so
/// the shape is spelled once and every column the post-commit validation
/// checks is checked before the stamp.
fn is_our_unattended_state_shape(shape: Option<(String, i32, Option<String>)>) -> bool {
    matches!(
        shape,
        Some((ref kind, 1, Some(ref default)))
            if kind.eq_ignore_ascii_case("integer") && default == "1"
    )
}

/// The two shapes v13 may have: `overlay` is `TEXT`, nullable, no default
/// (NULL reads as no overlay); `depth` is `INTEGER`, nullable, no default
/// (NULL reads as the closed end of the cap). Spelled once, like
/// [`is_our_unattended_state_shape`]: the v13 pre-stamp guard and the
/// post-commit check below both read these predicates. Neither can join
/// [`validate_profile_columns`], whose v12 pre-stamp call runs before the
/// v13 columns exist — checking them there refuses every fresh database.
fn is_our_overlay_shape(shape: Option<(String, i32, Option<String>)>) -> bool {
    matches!(
        shape,
        Some((ref kind, 0, None)) if kind.eq_ignore_ascii_case("text")
    )
}

fn is_our_depth_shape(shape: Option<(String, i32, Option<String>)>) -> bool {
    matches!(
        shape,
        Some((ref kind, 0, None)) if kind.eq_ignore_ascii_case("integer")
    )
}

/// The one shape v14 may have: `disowned_peer_session_id` is `TEXT`,
/// nullable, no default (NULL reads as no refusal recorded). Spelled once,
/// like its v13 siblings: the v14 pre-stamp guard and the post-commit check
/// both read this predicate.
fn is_our_disowned_shape(shape: Option<(String, i32, Option<String>)>) -> bool {
    matches!(
        shape,
        Some((ref kind, 0, None)) if kind.eq_ignore_ascii_case("text")
    )
}

fn validate_v14_columns(conn: &Connection) -> Result<(), JournalError> {
    if !is_our_disowned_shape(column_shape(conn, "disowned_peer_session_id")?) {
        return Err(JournalError::Corrupt(
            "journal schema has an unexpected sessions.disowned_peer_session_id column".to_string(),
        ));
    }
    Ok(())
}

fn validate_v13_columns(conn: &Connection) -> Result<(), JournalError> {
    if !is_our_overlay_shape(column_shape(conn, "overlay")?) {
        return Err(JournalError::Corrupt(
            "journal schema has an unexpected sessions.overlay column".to_string(),
        ));
    }
    if !is_our_depth_shape(column_shape(conn, "depth")?) {
        return Err(JournalError::Corrupt(
            "journal schema has an unexpected sessions.depth column".to_string(),
        ));
    }
    Ok(())
}

/// One column's `(type, notnull, default)` as SQLite reports it, or `None` when
/// the table has no such column.
fn column_shape(
    conn: &Connection,
    column: &str,
) -> Result<Option<(String, i32, Option<String>)>, JournalError> {
    let mut statement = conn.prepare(
        "SELECT type, \"notnull\", dflt_value FROM pragma_table_info('sessions') \
         WHERE name = ?1",
    )?;
    let shape = statement
        .query_row([column], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .optional()?;
    Ok(shape)
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
fn backfill_permission_origins(tx: &Connection) -> Result<(usize, usize, usize), JournalError> {
    let (candidates, unreadable, oversized) = {
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
        let mut oversized = 0usize;
        for row in rows {
            let (session_id, generation, seq, payload) = row?;
            match super::payload_with_origin(&payload) {
                super::OriginBackfill::Rewritten(_) => {
                    candidates.push((session_id, generation, seq))
                }
                // Already carries an origin, or is not a permission request.
                super::OriginBackfill::Nothing => {}
                super::OriginBackfill::Unreadable => unreadable += 1,
                // Past the read bound: not parsed, not rewritten, not touched.
                super::OriginBackfill::Oversized => oversized += 1,
            }
        }
        (candidates, unreadable, oversized)
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
            // the two passes — or that grew past the read bound — is left alone
            // rather than written blind.
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
    Ok((rewritten, unreadable, oversized))
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
#[path = "journal_schema_tests.rs"]
mod tests;

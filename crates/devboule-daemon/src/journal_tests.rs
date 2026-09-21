//! Tests for the journal: row durability, replay ordering and schema round-trips.

use super::*;
use devboule_protocol::TranscriptIntegrity;
use devboule_protocol::{UserMessageAuthor, UserMessageKind};
use std::process::Command;

/// §8 R2 / H7: the origin a *row* reads back as. `local` is the pre-v9
/// column default and stays local; a missing or unrecognised `kind` is
/// `Unknown`, which is not local and names no device. The `Daemon`
/// ownership arm reads `device_id` and requires `kind == Peer`, so a row
/// that cannot name the device that asked is refused exactly like a local
/// session instead of being promoted to one.
#[test]
fn a_stored_origin_kind_reads_back_as_local_peer_or_unknown() {
    let peer = origin_from_columns(
        Some("peer".to_string()),
        Some("device-phone".to_string()),
        Some("client".to_string()),
    );
    assert_eq!(peer.kind, SessionOriginKind::Peer);
    assert_eq!(peer.device_id.as_deref(), Some("device-phone"));
    assert_eq!(peer.role, Some(PeerRole::Client));
    assert!(!peer.is_local());

    assert!(origin_from_columns(Some("local".to_string()), None, None).is_local());
    // A NULL column is not a claim about who asked for the session.
    for kind in [None, Some("relay".to_string()), Some("".to_string())] {
        let unknown = origin_from_columns(kind.clone(), None, None);
        assert_eq!(
            unknown.kind,
            SessionOriginKind::Unknown,
            "kind {kind:?} is not local and is not peer"
        );
        assert!(!unknown.is_local());
        assert_eq!(unknown.device_id, None);
    }
    // Even with a device in the row: the kind is the whole answer, so an
    // unknown spelling names nobody.
    let unknown = origin_from_columns(
        Some("relay".to_string()),
        Some("device-x".to_string()),
        Some("daemon".to_string()),
    );
    assert_eq!(unknown.kind, SessionOriginKind::Unknown);
    assert_eq!(unknown.device_id, None);
    assert_eq!(unknown.role, None);

    // What the writer stores is what the reader maps back: the three
    // spellings round trip, including the one no writer produces yet.
    for origin in [
        SessionOrigin::local(),
        SessionOrigin::peer("device-phone", PeerRole::Daemon),
        SessionOrigin {
            kind: SessionOriginKind::Unknown,
            device_id: None,
            role: None,
        },
    ] {
        let columns = (
            Some(origin_kind_str(&origin).to_string()),
            origin.device_id.clone(),
            origin.role.map(|role| role.as_str().to_string()),
        );
        assert_eq!(origin_from_columns(columns.0, columns.1, columns.2), origin);
    }
}

#[test]
fn journal_test_directory_does_not_reuse_pid_counter_candidate() {
    let process_id = std::process::id();
    let mut candidates = Vec::new();
    let mut created = Vec::new();
    for counter in 1..=256 {
        let dir = std::env::temp_dir().join(format!("devboule journal {process_id}-{counter}"));
        candidates.push(dir.clone());
        if std::fs::create_dir(&dir).is_ok() {
            created.push(dir);
        }
    }

    let (selected, _) = tmp_journal();
    let reused = candidates.iter().any(|dir| dir == &selected);
    let selected_display = selected.display().to_string();
    if !reused {
        let _ = std::fs::remove_dir_all(&selected);
    }
    for dir in created {
        let _ = std::fs::remove_dir_all(dir);
    }
    assert!(
        !reused,
        "reused legacy journal test directory: {selected_display}"
    );
}

#[test]
fn shutdown_joins_the_writer_even_with_a_saturated_queue() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    // Saturate the data-queue cap without sending anything. The old
    // `shutdown` reserved a data slot for `Shutdown` with a 200 ms
    // budget and silently dropped the failure, so the writer (and its
    // SQLite connection) stayed alive past `shutdown`.
    for _ in 0..JOURNAL_QUEUE_CAP {
        journal.reserve_slot();
    }
    assert!(
        !journal.reserve_slot(),
        "the queue cap is saturated for the shutdown below"
    );
    journal.shutdown();
    for _ in 0..JOURNAL_QUEUE_CAP {
        journal.release_slot();
    }
    // Barrier, cross-platform proof: the writer thread is gone, so a
    // blocking append is refused instead of accepted.
    assert!(
        matches!(
            journal.append_blocking(output_record("s.shutdown", 1, 1, b"late")),
            Err(JournalError::Stopped)
        ),
        "append after shutdown must fail: the writer is gone"
    );
    // Barrier, Windows proof: with the connection closed the database
    // directory removes immediately, with no retry.
    std::fs::remove_dir_all(&dir).expect("writer is joined: immediate removal");
}

#[test]
fn project_and_workspace_rows_round_trip_and_project_add_is_idempotent() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("journal");
    let project = ProjectRecord {
        id: "p.first".to_string(),
        name: "Project With Spaces".to_string(),
        path: r"C:\Users\alice\Project With Spaces".to_string(),
        git_state: "inside_repository".to_string(),
        created_at_ms: 10,
        updated_at_ms: 10,
    };
    let inserted = journal
        .project_add(project.clone())
        .expect("insert project");
    assert_eq!(inserted, project);

    let refreshed = journal
        .project_add(ProjectRecord {
            id: "p.second".to_string(),
            name: "Project Refreshed".to_string(),
            git_state: "repository".to_string(),
            updated_at_ms: 11,
            ..project.clone()
        })
        .expect("refresh same path");
    let expected = ProjectRecord {
        id: "p.first".to_string(),
        name: "Project Refreshed".to_string(),
        git_state: "repository".to_string(),
        updated_at_ms: 11,
        ..project.clone()
    };
    assert_eq!(refreshed.id, "p.first");
    assert_eq!(
        journal.projects_list().expect("list projects"),
        vec![expected.clone()]
    );

    let workspace = WorkspaceRecord {
        id: "w.local".to_string(),
        project_id: project.id.clone(),
        title: project.name.clone(),
        isolation: WorkspaceIsolation::Local,
        path: project.path.clone(),
        branch: None,
        created_at_ms: 12,
        updated_at_ms: 12,
    };
    assert_eq!(
        journal
            .workspace_create(workspace.clone())
            .expect("insert workspace"),
        workspace
    );
    assert_eq!(
        journal
            .workspaces_list(&project.id)
            .expect("list workspaces"),
        vec![workspace.clone()]
    );
    assert_eq!(
        journal.workspace_get(&workspace.id).expect("get workspace"),
        Some(workspace)
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn project_wire_path_is_human_readable_but_storage_keeps_verbatim_path() {
    let project = ProjectRecord {
        id: "p.display".to_string(),
        name: "Project".to_string(),
        path: r"\\?\C:\Users\alice\Project".to_string(),
        git_state: "unknown".to_string(),
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    assert_eq!(project.path, r"\\?\C:\Users\alice\Project");
    assert_eq!(project.to_project().path, r"C:\Users\alice\Project");
}

#[test]
fn records_without_terminators_are_always_unverifiable() {
    for status in [PersistStatus::Interrupted, PersistStatus::Live] {
        let mut record = sample_session("s.unverifiable");
        record.status = status;

        assert_eq!(
            record.to_session().state,
            SessionState::Recovered {
                generation: 1,
                integrity: TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    trimmed_bytes: 0,
                },
            }
        );
    }
}

#[test]
fn to_session_carries_the_resume_verdict_from_the_trait() {
    // A dead admitted row with its columns offers resume; without them,
    // or of an undesigned family, it does not. The app renders this bool
    // and never re-derives it.
    let mut claude = sample_session("s.claude.dead");
    claude.kind = SessionKind::Claude;
    claude.status = PersistStatus::Ended;
    claude.provider = Some("claude".to_string());
    claude.peer_session_id = Some("peer-1".to_string());
    assert!(claude.to_session().resumable);

    let mut columnless = sample_session("s.claude.nocols");
    columnless.kind = SessionKind::Claude;
    columnless.status = PersistStatus::Ended;
    assert!(!columnless.to_session().resumable);

    let mut pi = sample_session("s.pi.dead");
    pi.kind = SessionKind::Pi;
    pi.status = PersistStatus::Ended;
    pi.provider = Some("pi".to_string());
    pi.peer_session_id = Some("peer-1".to_string());
    assert!(pi.to_session().resumable);

    let mut terminal = sample_session("s.terminal.dead");
    terminal.kind = SessionKind::Terminal;
    terminal.status = PersistStatus::Ended;
    terminal.provider = Some("terminal".to_string());
    terminal.peer_session_id = Some("peer-1".to_string());
    assert!(!terminal.to_session().resumable);
}

#[test]
fn file_len_accounts_for_an_uncheckpointed_wal() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("journal");
    journal
        .upsert_blocking(sample_session("s.wal.size"))
        .expect("session");

    assert!(journal.try_append(output_record("s.wal.size", 1, 1, vec![b'x'; 256 * 1024],)));
    let reported = journal.file_len().expect("reported journal size");
    let main_bytes = std::fs::metadata(&path).expect("main journal").len();
    let wal_path = path.with_file_name("journal.db-wal");
    let wal_bytes = std::fs::metadata(&wal_path).expect("wal journal").len();

    assert!(wal_bytes > 0, "the test must observe an uncheckpointed WAL");
    assert!(
        reported >= main_bytes.saturating_add(wal_bytes),
        "reported={reported} main={main_bytes} wal={wal_bytes}"
    );

    journal.shutdown();
    // Best-effort, like every other test here: shutdown gives the worker a
    // budget and moves on, so the database may still be open on Windows.
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn interrupted_loss_keeps_counters_but_not_certification() {
    let mut record = sample_session("s.unverifiable.loss");
    record.status = PersistStatus::Interrupted;
    record.degraded = true;
    record.dropped_frames = 7;
    record.dropped_bytes = 4096;

    assert_eq!(
        record.to_session().state,
        SessionState::Recovered {
            generation: 1,
            integrity: TranscriptIntegrity::Unverifiable {
                dropped_frames: 7,
                dropped_bytes: 4096,
                trimmed_bytes: 0,
            },
        }
    );
}

/// Audit S5-12: the name and the parent of an agent-created session are the
/// journal's, so a restart brings both back.
///
/// A row with neither stays NULL — every session a human asked for — and
/// the surfaces fall back to the title exactly as they did before the
/// columns existed.
#[test]
fn a_display_name_and_a_creator_survive_a_restart_and_an_absent_one_stays_null() {
    let (dir, path) = tmp_journal();
    {
        let journal = Journal::open(&path).expect("open");
        let mut child = sample_session("s.child");
        child.display_name = Some("builder".to_string());
        child.created_by = Some("s.parent".to_string());
        journal.upsert_blocking(child).expect("upsert the child");
        journal
            .upsert_blocking(sample_session("s.human"))
            .expect("upsert a session nobody created");
        journal.shutdown();
    }

    // A second open is the restart: the migration is where a database that
    // predates the columns gets them, and this read is what proves the
    // *values* came back rather than only the schema.
    let journal = Journal::open(&path).expect("reopen");
    let rows = journal.list().expect("list");
    let child = rows
        .iter()
        .find(|row| row.id == "s.child")
        .expect("the child's row survived the restart");
    assert_eq!(child.display_name.as_deref(), Some("builder"));
    assert_eq!(child.created_by.as_deref(), Some("s.parent"));
    assert_eq!(
        child.to_session().display_name.as_deref(),
        Some("builder"),
        "a recovered transcript lists under the name the human saw"
    );
    assert_eq!(child.to_session().created_by.as_deref(), Some("s.parent"));
    let human = rows
        .iter()
        .find(|row| row.id == "s.human")
        .expect("the human's row");
    assert!(
        human.display_name.is_none() && human.created_by.is_none(),
        "a session a human asked for has neither, and NULL is how that is said"
    );
    // An upsert that carries neither must not erase what the create wrote:
    // the row is the same session, and the finish of its first generation
    // does not know its name.
    let mut update = sample_session("s.child");
    update.display_name = None;
    update.created_by = None;
    journal.upsert_blocking(update).expect("upsert again");
    let again = journal
        .list()
        .expect("list again")
        .into_iter()
        .find(|row| row.id == "s.child")
        .expect("still there");
    assert_eq!(
        again.display_name.as_deref(),
        Some("builder"),
        "a later upsert without a name keeps the one the create wrote"
    );
    assert_eq!(again.created_by.as_deref(), Some("s.parent"));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ended_loss_replays_degraded_before_exit_and_reports_truncated() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let mut record = sample_session("s.ended.loss");
    record.status = PersistStatus::Ended;
    record.degraded = true;
    record.dropped_frames = 3;
    record.dropped_bytes = 4096;
    journal.upsert_blocking(record).expect("upsert");

    let replay = journal.replay("s.ended.loss").expect("replay");
    assert_eq!(
        replay.events,
        vec![
            SessionEvent::JournalDegraded {
                dropped_frames: 3,
                dropped_bytes: 4096,
            },
            SessionEvent::Exit { code: None },
        ]
    );
    assert_eq!(
        replay.integrity,
        TranscriptIntegrity::Truncated {
            dropped_frames: 3,
            dropped_bytes: 4096,
            trimmed_bytes: 0,
        }
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ended_clean_replays_exit_only_and_reports_complete() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let mut record = sample_session("s.ended.clean");
    record.status = PersistStatus::Ended;
    journal.upsert_blocking(record).expect("upsert");

    let replay = journal.replay("s.ended.clean").expect("replay");
    assert_eq!(replay.events, vec![SessionEvent::Exit { code: None }]);
    assert_eq!(replay.integrity, TranscriptIntegrity::Complete);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn enqueue_drops_count_the_exact_payload_sizes() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.drop").clone())
        .expect("upsert");
    let blocker = Connection::open(&path).expect("blocker");
    blocker
        .execute_batch("BEGIN EXCLUSIVE")
        .expect("hold sqlite writer lock");

    while journal.queued.load(Ordering::Acquire)
        < JOURNAL_QUEUE_CAP.saturating_sub(CONTROL_RESERVE) as u64
    {
        assert!(journal.try_append(output_record("s.drop", 1, 1, b"fill")));
    }
    let baseline = journal
        .degraded_sessions
        .lock()
        .expect("degradation baseline")
        .get("s.drop")
        .copied()
        .unwrap_or_default();
    assert!(!journal.try_append(output_record("s.drop", 1, 2, b"abc")));
    assert!(!journal.try_append(output_record("s.drop", 1, 3, b"12345678")));

    let counters = journal
        .degraded_sessions
        .lock()
        .expect("degradation counters")
        .get("s.drop")
        .copied()
        .expect("dropped session");
    assert_eq!(counters.frames - baseline.frames, 2);
    assert_eq!(counters.bytes - baseline.bytes, 3 + 8);

    drop(blocker);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn permission_decision_is_written_with_outcome_timestamp_and_payload() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.permission.audit"))
        .expect("session row");
    let payload = br#"{"type":"permission_request","toolCallId":"tool-1"}"#;
    journal
        .record_permission("s.permission.audit", "tool-1", "allow_once", payload)
        .expect("permission row");
    journal.flush().expect("permission flush");
    let conn = Connection::open(&path).expect("inspect");
    let row = conn
        .query_row(
            "SELECT ts_ms, outcome, payload, checksum FROM permissions
                 WHERE session_id = ?1 AND request_id = ?2",
            ["s.permission.audit", "tool-1"],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .expect("permission row");
    assert!(row.0 > 0);
    assert_eq!(row.1, "allow_once");
    assert_eq!(row.2, payload);
    assert_eq!(row.3, crc32(payload) as i64);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn permission_reuse_is_rejected_without_overwriting_the_audit_row() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.permission.reuse"))
        .expect("session row");
    journal
        .record_permission(
            "s.permission.reuse",
            "tool-1",
            "allow_once",
            br#"{"decision":1}"#,
        )
        .expect("first permission row");
    assert!(journal
        .record_permission("s.permission.reuse", "tool-1", "deny", br#"{"decision":2}"#,)
        .is_err());
    journal.flush().expect("permission flush");
    let conn = Connection::open(&path).expect("inspect");
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM permissions WHERE session_id = ?1 AND request_id = ?2",
            ["s.permission.reuse", "tool-1"],
            |row| row.get(0),
        )
        .expect("permission rows");
    assert_eq!(rows, 1);
    let row: (String, Vec<u8>) = conn
        .query_row(
            "SELECT outcome, payload FROM permissions
                 WHERE session_id = ?1 AND request_id = ?2",
            ["s.permission.reuse", "tool-1"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("original permission row");
    assert_eq!(row.0, "allow_once");
    assert_eq!(row.1, br#"{"decision":1}"#);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn append_and_replay_preserves_seq() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.a.1"))
        .expect("upsert");
    journal
        .append_blocking(output_record("s.a.1", 1, 1, b"one"))
        .expect("a");
    journal
        .append_blocking(output_record("s.a.1", 1, 2, b"two"))
        .expect("b");
    let replay = journal.replay("s.a.1").expect("replay");
    match &replay.events[..] {
        [SessionEvent::Output { seq: 1, data: a }, SessionEvent::Output { seq: 2, data: b }, SessionEvent::Recovered {
            integrity:
                TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    ..
                },
        }] => {
            assert_eq!(a, "one");
            assert_eq!(b, "two");
        }
        other => panic!("unexpected replay: {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn agent_report_survives_replay() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.a.1"))
        .expect("upsert");
    journal
        .append_blocking(output_record("s.a.1", 1, 1, b"one"))
        .expect("output");
    let event = SessionEvent::AgentReported {
        seq: 2,
        source: "devboule:stub".to_string(),
        agent: "stub".to_string(),
        state: devboule_protocol::AgentActivityState::Working,
        message: None,
        report_seq: Some(7),
        agent_session_id: Some("agent-1".to_string()),
        agent_session_path: None,
        session_start_source: Some("startup".to_string()),
    };
    journal
        .append_blocking(agent_report_record("s.a.1", 1, 2, &event).expect("record"))
        .expect("report");
    let replay = journal.replay("s.a.1").expect("replay");
    assert!(
        replay.events.iter().any(|item| item == &event),
        "replay missing agent report: {:?}",
        replay.events
    );
    let payload = serde_json::to_vec(&event).expect("payload");
    let stored = journal
        .list()
        .expect("list")
        .into_iter()
        .find(|row| row.id == "s.a.1")
        .expect("session");
    assert_eq!(
        stored.payload_bytes,
        b"one".len() as u64 + payload.len() as u64,
        "agent_report must count toward payload_bytes"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The `preset` → `profile` rename (audit F2): a row journaled before v11
/// carries `preset`, and the replay reader parses the payload in one
/// tolerant `if let Ok` — an unparseable row is dropped with no counter.
/// The alias on `AgentCreated.profile` is what keeps this row hydrating.
///
/// The bytes are what a pre-v11 daemon wrote, `preset` word and all, and
/// the replayed event keeps the preset value under `profile` untranslated:
/// the journal keeps saying what it said, and the transcript shows a
/// creation record rather than silence.
#[test]
fn a_preset_spelled_agent_created_row_hydrates_on_replay() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.a.1"))
        .expect("upsert");
    let legacy = serde_json::json!({
        "type": "agent_created",
        "messageId": "devboule-agent-created-1-2",
        "childSessionId": "s.child.1",
        "displayName": "Poster",
        "provider": "claude",
        "preset": "design",
    });
    let record = EventRecord {
        session_id: "s.a.1".to_string(),
        generation: 1,
        seq: 1,
        kind: EventKind::AgentReport,
        ts_ms: now_ms(),
        payload: serde_json::to_vec(&legacy).expect("legacy payload"),
    };
    journal.append_blocking(record).expect("append");
    let replay = journal.replay("s.a.1").expect("replay");
    let created = replay
        .events
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentCreated { profile, .. } => Some(profile.clone()),
            _ => None,
        })
        .expect("the legacy agent_created row must hydrate, not vanish");
    assert_eq!(
        created, "design",
        "the preset value survives under `profile`, untranslated"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same rename, one struct over: a creation card rides inside a
/// `permission_request` row as a nested required object, so a card
/// journalled before `d5c72a3` spelled `preset` — and without the alias
/// the whole permission row fails to decode and is dropped with no
/// counter, taking the human's record of the card with it. The bytes are
/// what a pre-rename daemon wrote: `preset` word, and no `tools` (a later
/// addition, rescued by its `#[serde(default)]`).
#[test]
fn a_preset_spelled_creation_card_hydrates_on_replay() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.p.card"))
        .expect("upsert");
    let current = SessionEvent::PermissionRequest {
        tool_call_id: "devboule-create-1".to_string(),
        title: "Create an agent: Poster (design)".to_string(),
        description: None,
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![devboule_protocol::PermissionOption {
            option_id: "allow".to_string(),
            name: "Allow".to_string(),
            kind: "allow_once".to_string(),
        }],
        origin: devboule_protocol::SessionOrigin::local(),
        create_agent: Some(devboule_protocol::CreateAgentCard {
            creator_session_id: "s.p.card".to_string(),
            provider: "claude".to_string(),
            profile: "design".to_string(),
            title: "Poster".to_string(),
            tools: "unverified".to_string(),
            caps: devboule_protocol::CreateAgentCaps {
                live_children: 1,
                max_live_children: 3,
                creations_this_hour: 1,
                max_creations_per_hour: 20,
                depth: 1,
                max_depth: 3,
                live_agent_sessions: 1,
                max_live_agent_sessions: 8,
            },
        }),
    };
    // The rename, applied to the serialized frame: exactly the bytes an
    // older daemon journalled, before `profile` existed and before `tools`
    // did.
    let mut legacy = serde_json::to_value(&current).expect("serialize current event");
    let card = legacy
        .get_mut("createAgent")
        .and_then(|card| card.as_object_mut())
        .expect("createAgent object");
    let profile = card.remove("profile").expect("profile key");
    card.insert("preset".to_string(), profile);
    card.remove("tools");
    let record = EventRecord {
        session_id: "s.p.card".to_string(),
        generation: 1,
        seq: 1,
        kind: EventKind::AgentReport,
        ts_ms: now_ms(),
        payload: serde_json::to_vec(&legacy).expect("legacy payload"),
    };
    journal.append_blocking(record).expect("append");
    let replay = journal.replay("s.p.card").expect("replay");
    let hydrated = replay
        .events
        .iter()
        .find_map(|event| match event {
            SessionEvent::PermissionRequest {
                create_agent: Some(card),
                ..
            } => Some(card.clone()),
            _ => None,
        })
        .expect("the legacy creation card must hydrate, not vanish");
    assert_eq!(
        hydrated.profile, "design",
        "the preset value survives under `profile`, untranslated"
    );
    assert_eq!(
        hydrated.tools, "unverified",
        "the absent tools word decodes as the not-established default"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn acp_envelopes_do_not_leave_unsnapshotted_bytes_stuck() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open_with_limits(
        &path,
        JournalLimits {
            snapshot_every_bytes: 8,
            ..JournalLimits::default()
        },
    )
    .expect("open");
    let mut session = sample_session("s.acp.snap");
    session.kind = SessionKind::Acp;
    journal.upsert_blocking(session).expect("upsert");
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "s",
            "update": {
                "sessionUpdate": "agent_thought_chunk",
                "content": {"type": "text", "text": "The"}
            }
        }
    });
    for seq in 1..=4 {
        journal
            .append_blocking(acp_envelope_record("s.acp.snap", 1, seq, &envelope).expect("rec"))
            .expect("append");
    }
    journal.flush().expect("flush");
    let conn = Connection::open(&path).expect("inspect");
    let unsnapshotted: i64 = conn
        .query_row(
            "SELECT unsnapshotted_bytes FROM sessions WHERE id = ?1",
            ["s.acp.snap"],
            |row| row.get(0),
        )
        .expect("unsnapshotted");
    assert_eq!(
        unsnapshotted, 0,
        "ACP envelopes must not accumulate unsnapshotted_bytes"
    );
    let snapshots: i64 = conn
        .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
        .expect("snapshots");
    assert_eq!(
        snapshots, 0,
        "ACP envelopes must not create output snapshots"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn snapshot_compacts_and_replay_has_no_gap_or_duplicate() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open_with_limits(&path, snapshot_limits()).expect("open");
    journal
        .upsert_blocking(sample_session("s.a.1"))
        .expect("upsert");
    for seq in 1..=8 {
        journal
            .append_blocking(output_record(
                "s.a.1",
                1,
                seq,
                format!("chunk-{seq:02}....").as_bytes(),
            ))
            .expect("append");
    }
    let replay = journal.replay("s.a.1").expect("replay");
    let seqs: Vec<u64> = replay
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    let conn = Connection::open(&path).expect("read");
    let snaps: i64 = conn
        .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
        .expect("snaps");
    assert!(snaps >= 1, "expected at least one snapshot, got {snaps}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn degradation_is_scoped_to_a_session_and_journal_lifetime() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.a.1"))
        .expect("upsert");
    assert!(!journal.is_session_degraded("s.a.1"));
    assert!(journal.note_session_degraded("s.a.1"));
    assert!(journal.is_session_degraded("s.a.1"));
    assert!(!journal.is_session_degraded("s.a.2"));
    assert!(!journal.note_session_degraded("s.a.1"));
    journal.flush().expect("degradation marker");
    assert!(journal
        .list()
        .expect("list")
        .into_iter()
        .find(|row| row.id == "s.a.1")
        .is_some_and(|row| row.degraded));
    drop(journal);

    let journal = Journal::open(&path).expect("reopen");
    assert!(!journal.is_session_degraded("s.a.1"));
    assert!(journal
        .list()
        .expect("reopen list")
        .into_iter()
        .find(|row| row.id == "s.a.1")
        .is_some_and(|row| row.degraded));
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn poisoned_degradation_set_is_fail_closed() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.poisoned"))
        .expect("upsert");
    let poisoned = Arc::clone(&journal.degraded_sessions);
    let panic = std::thread::spawn(move || {
        let _sessions = poisoned.lock().expect("degradation lock");
        panic!("simulate a journal-state panic");
    });
    assert!(panic.join().is_err());

    assert!(journal.note_session_degraded("s.poisoned"));
    assert!(journal.is_session_degraded("s.poisoned"));
    journal.flush().expect("degradation marker");
    assert!(journal
        .list()
        .expect("list")
        .into_iter()
        .find(|row| row.id == "s.poisoned")
        .is_some_and(|row| row.degraded));
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn uncommitted_write_is_not_visible_after_reopen() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.a.1"))
        .expect("upsert");
    drop(journal);
    let conn = Connection::open(&path).expect("raw");
    conn.execute("BEGIN", []).expect("begin");
    conn.execute(
        "INSERT INTO events (session_id, generation, seq, kind, ts_ms, payload, checksum)
             VALUES ('s.a.1', 1, 99, 'output', 1, X'00', 0)",
        [],
    )
    .expect("insert");
    drop(conn);
    let journal = Journal::open(&path).expect("reopen");
    let replay = journal.replay("s.a.1").expect("replay");
    assert!(replay.events.iter().all(|event| match event {
        SessionEvent::Output { seq, .. } => *seq != 99,
        _ => true,
    }));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A row whose `kind` no writer in this binary knows must fail the replay
/// rather than vanish from it.
///
/// The read is followed by a verdict the hole cannot reach: `last_seq` is the
/// sessions row's own value and `TranscriptIntegrity` is computed from the
/// session's columns, so a skipped row leaves the transcript short by one and
/// still reporting `Complete`. A foreign writer (a downgrade, above all) is
/// the only way to get such a row; the daemon's own writers cannot skip it.
#[test]
fn an_unknown_event_kind_fails_the_replay_instead_of_vanishing() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.foreign-kind"))
        .expect("upsert");
    journal
        .append_blocking(output_record("s.foreign-kind", 1, 1, b"kept"))
        .expect("output");
    journal
        .append_blocking(output_record("s.foreign-kind", 1, 2, b"foreign"))
        .expect("the row a later build writes with a kind this one does not know");
    journal.try_mark_ended("s.foreign-kind", 1, Some(0));
    journal.flush().expect("flush ended");
    // The verdict the skip hides behind: a terminated session with no dropped
    // frame and no trimmed byte says `Complete` about a transcript that is
    // short by one row.
    let intact = journal.replay("s.foreign-kind").expect("replay");
    assert_eq!(
        intact.integrity,
        TranscriptIntegrity::Complete,
        "the fixture's own verdict must be the one the skip masks"
    );
    drop(journal);
    // Only the kind is rewritten, by hand: the checksum covers the payload, so
    // this is byte-for-byte what a downgraded build reads.
    let conn = Connection::open(&path).expect("raw");
    let rewritten = conn
        .execute(
            "UPDATE events SET kind = 'agent_report_x'
                 WHERE session_id = 's.foreign-kind' AND generation = 1 AND seq = 2",
            [],
        )
        .expect("rewrite the kind");
    assert_eq!(rewritten, 1, "the fixture rewrote exactly one row");
    drop(conn);

    let journal = Journal::open(&path).expect("reopen");
    let error = journal
        .replay("s.foreign-kind")
        .expect_err("an unknown event kind must not be skipped in silence");
    assert!(
        matches!(error, JournalError::Corrupt(ref message) if message.contains("agent_report_x")),
        "the refusal names the row: {error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn drain_output_after_process_exit_is_not_dropped() {
    // ConPTY keeps delivering after Child::wait.
    // Marking the journal ended at wait-time steals last_seq+1 for the
    // exit row; the drain frame then collides and vanishes. This is the
    // silent tail loss: live ring has the bytes, replay does not.
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.drain.1"))
        .expect("upsert");
    journal
        .append_blocking(output_record("s.drain.1", 1, 1, b"HEAD"))
        .expect("head");
    // Waiter path: process reaped, output still live.
    journal.try_mark_reaped("s.drain.1", Some(0));
    journal.flush().expect("flush reaped");
    let drain = vec![b'X'; 3953];
    journal
        .append_blocking(output_record("s.drain.1", 1, 2, &drain))
        .expect("drain append must succeed; an exit row must not occupy seq 2");
    // EOF path: now freeze last_seq with the exit row.
    journal.try_mark_ended("s.drain.1", 1, Some(0));
    journal.flush().expect("flush ended");
    let replay = journal.replay("s.drain.1").expect("replay");
    let output: String = replay
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { data, .. } => Some(data.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        output.len(),
        4 + 3953,
        "journal silently lost the drain tail: {output:?}"
    );
    assert!(output.starts_with("HEAD"), "{output:?}");
    assert!(output.ends_with(&"X".repeat(3953)), "drain tail missing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ended_and_interrupted_replay_are_distinct() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .upsert_blocking(sample_session("s.ended"))
        .expect("upsert");
    journal
        .append_blocking(output_record("s.ended", 1, 1, b"bye"))
        .expect("out");
    journal
        .send_cmd(
            JournalCmd::MarkEnded {
                session_id: "s.ended".to_string(),
                generation: 1,
                code: Some(0),
            },
            RPC_WAIT,
        )
        .expect("mark");
    journal.flush().expect("flush");
    journal
        .upsert_blocking(sample_session("s.kill"))
        .expect("kill");
    journal
        .append_blocking(output_record("s.kill", 1, 1, b"still"))
        .expect("out");
    drop(journal);

    let journal = Journal::open(&path).expect("reopen");
    let ended = journal.replay("s.ended").expect("ended");
    let killed = journal.replay("s.kill").expect("killed");
    assert!(matches!(
        ended.events.last(),
        Some(SessionEvent::Exit { code: Some(0) })
    ));
    assert!(matches!(
        killed.events.last(),
        Some(SessionEvent::Recovered {
            integrity: TranscriptIntegrity::Unverifiable {
                dropped_frames: 0,
                dropped_bytes: 0,
                ..
            },
        })
    ));
    let listed = journal.list().expect("list");
    assert!(listed.iter().any(|row| {
        row.id == "s.ended" && matches!(row.to_session().state, SessionState::Ended { .. })
    }));
    assert!(listed.iter().any(|row| {
        row.id == "s.kill" && matches!(row.to_session().state, SessionState::Recovered { .. })
    }));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unclean_reopen_reports_doubt_not_completeness() {
    // The cross the protocol contract requires: measured missing bytes
    // and the honest flags must agree. After a process death the
    // database is the only witness, and it cannot know what was still
    // uncommitted in the dying writer queue, so:
    //
    // - a session whose journal was not closed orderly is ALWAYS
    //   Recovered, whatever the degraded column says: Recovered means
    //   "tail unverifiable";
    // - no terminator is always Unverifiable, with any measured loss
    //   carried as counters. A terminator is required before Truncated
    //   can certify the remaining tail.
    let (dir, path) = tmp_journal();

    // Silent queue death: five frames committed, five more produced
    // into the queue and lost with the process. No failure was ever
    // observed, so no truncation may be claimed — but the session
    // must be Recovered, never presented as complete.
    {
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.cross.silent"))
            .expect("upsert");
        for seq in 1..=5 {
            journal
                .append_blocking(output_record("s.cross.silent", 1, seq, b"data"))
                .expect("append");
        }
        // The drop simulates the kill: the row stays status=live, so
        // the reopen sees a journal nobody closed orderly.
        drop(journal);
    }
    {
        let journal = Journal::open(&path).expect("reopen");
        let listed = journal.list().expect("list");
        let row = listed
            .iter()
            .find(|row| row.id == "s.cross.silent")
            .expect("session row");
        assert!(matches!(
            row.to_session().state,
            SessionState::Recovered {
                integrity: TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    ..
                },
                ..
            }
        ));
        let replay = journal.replay("s.cross.silent").expect("replay");
        let replay_bytes: usize = replay
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output { data, .. } => Some(data.len()),
                _ => None,
            })
            .sum();
        assert_eq!(replay_bytes, 20, "committed frames must replay intact");
        assert!(matches!(
            replay.events.last(),
            Some(SessionEvent::Recovered {
                integrity: TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    ..
                },
            })
        ));
        assert!(
            matches!(
                replay.integrity,
                TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    ..
                }
            ),
            "an unclosed journal must stay unverifiable"
        );
    }

    // Observed loss: the same five committed frames, but the previous
    // daemon recorded degradation (queue pressure). The tail is still
    // unverifiable because no terminator committed.
    {
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.cross.declared"))
            .expect("upsert");
        for seq in 1..=5 {
            journal
                .append_blocking(output_record("s.cross.declared", 1, seq, b"data"))
                .expect("append");
        }
        journal.note_session_degraded("s.cross.declared");
        journal.flush().expect("degradation marker");
        drop(journal);
    }
    {
        let journal = Journal::open(&path).expect("reopen");
        let listed = journal.list().expect("list");
        let row = listed
            .iter()
            .find(|row| row.id == "s.cross.declared")
            .expect("session row");
        assert!(matches!(
            row.to_session().state,
            SessionState::Recovered {
                integrity: TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    ..
                },
                ..
            }
        ));
        let replay = journal.replay("s.cross.declared").expect("replay");
        assert_eq!(
            replay.integrity,
            TranscriptIntegrity::Unverifiable {
                dropped_frames: 0,
                dropped_bytes: 0,
                trimmed_bytes: 0,
            }
        );
        assert!(matches!(
            replay.events.last(),
            Some(SessionEvent::Recovered {
                integrity: TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    ..
                },
            })
        ));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hammer_writer_loop() {
    let Ok(path) = std::env::var("DEVBOULE_JOURNAL_HAMMER_PATH") else {
        return;
    };
    let journal = Journal::open(Path::new(&path)).expect("hammer open");
    journal
        .upsert_blocking(sample_session("s.hammer.1"))
        .expect("upsert");
    let ready = Path::new(&path).with_file_name("hammer.ready");
    std::fs::write(&ready, b"ok").expect("ready");
    let payload = vec![b'x'; 4096];
    let mut seq = 1u64;
    loop {
        let _ = journal.tx.send(JournalCmd::Append(output_record(
            "s.hammer.1",
            1,
            seq,
            &payload,
        )));
        seq = seq.saturating_add(1);
    }
}

#[test]
fn kill_writer_mid_append_journal_is_readable() {
    if std::env::var_os("DEVBOULE_JOURNAL_HAMMER_PATH").is_some() {
        return;
    }
    let (dir, path) = tmp_journal();
    {
        let journal = Journal::open(&path).expect("seed");
        drop(journal);
    }
    let exe = std::env::current_exe().expect("exe");
    let mut child = Command::new(&exe)
        .env("DEVBOULE_JOURNAL_HAMMER_PATH", &path)
        .args([
            "--exact",
            "--test-threads=1",
            "journal::tests::hammer_writer_loop",
        ])
        .spawn()
        .expect("spawn hammer");
    let ready = path.with_file_name("hammer.ready");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    if !ready.exists() {
        let _ = child.kill();
        let status = child.wait();
        panic!("hammer child did not start writing; wait={status:?}");
    }
    std::thread::sleep(Duration::from_millis(300));
    let _ = child.kill();
    let _ = child.wait();
    let journal = Journal::open(&path).expect("reopen after kill");
    let replay = journal.replay("s.hammer.1");
    assert!(replay.is_ok(), "journal unreadable after kill: {replay:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn peer_record(device_id: &str) -> PeerRecord {
    PeerRecord {
        device_id: device_id.to_string(),
        display_name: "Marco's MacBook Pro".to_string(),
        role: "daemon".to_string(),
        public_key: vec![7u8; 32],
        paired_by_user: Some("S-1-5-21-1".to_string()),
        binding_kind: "tailnet".to_string(),
        binding_stable_id: Some("nxd5gUfvzj11CNTRL".to_string()),
        binding_node_name: Some("marcos-macbook-pro.tail80a42d.ts.net.".to_string()),
        binding_login_name: Some("user@example.com".to_string()),
        address: "100.74.116.126:47831".to_string(),
        paired_at: 1_700_000_000_000,
        revoked_at: None,
        caps: devboule_protocol::PEER_DEFAULT_CAPS
            .iter()
            .map(|cap| cap.to_string())
            .collect(),
    }
}

fn audit_record(device_id: &str, action: &str) -> AuditRecord {
    AuditRecord {
        device_id: device_id.to_string(),
        role: "daemon".to_string(),
        claimed_origin: None,
        action: action.to_string(),
        session_id: None,
        outcome: "ok".to_string(),
    }
}

#[test]
fn peers_round_trip_revoke_and_caps() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let record = peer_record("dev-1");
    let stored = journal.peer_upsert(record.clone()).expect("upsert");
    assert_eq!(stored, record);
    assert_eq!(journal.peers_list().expect("list").len(), 1);
    let loaded = journal.peer_get("dev-1").expect("get").expect("row");
    assert_eq!(loaded.role, "daemon");
    assert_eq!(loaded.caps, vec!["view".to_string()]);
    assert!(loaded.owns_address(&"100.74.116.126".parse().expect("ip")));
    assert!(!loaded.owns_address(&"100.74.116.127".parse().expect("ip")));

    assert_eq!(
        journal
            .peer_set_caps("dev-1", vec!["view".into(), "send".into()])
            .expect("caps"),
        PeerMutation::Updated
    );
    assert_eq!(
        journal.peer_get("dev-1").expect("get").expect("row").caps,
        vec!["view".to_string(), "send".to_string()]
    );
    assert_eq!(
        journal.peer_set_caps("missing", vec![]).expect("caps"),
        PeerMutation::NotFound
    );

    assert_eq!(
        journal.peer_revoke("dev-1", 42).expect("revoke"),
        PeerMutation::Updated
    );
    assert_eq!(
        journal.peer_revoke("dev-1", 43).expect("revoke twice"),
        PeerMutation::Revoked,
        "a second revoke says the row was already revoked, not that it is missing"
    );
    assert_eq!(
        journal.peer_revoke("missing", 43).expect("revoke unknown"),
        PeerMutation::NotFound
    );
    assert!(journal
        .peer_get("dev-1")
        .expect("get")
        .expect("row")
        .is_revoked());
    // And a revoked row's capabilities cannot be rewritten (C8).
    assert_eq!(
        journal
            .peer_set_caps("dev-1", vec!["view".into()])
            .expect("caps on a revoked row"),
        PeerMutation::Revoked
    );

    // A re-pair after revoke replaces the row, including cleared
    // revocation, and does not duplicate it.
    let mut again = record;
    again.display_name = "Renamed".to_string();
    again.revoked_at = None;
    let replaced = journal.peer_upsert(again).expect("re-upsert");
    assert_eq!(replaced.display_name, "Renamed");
    assert!(!replaced.is_revoked());
    assert_eq!(journal.peers_list().expect("list").len(), 1);

    let mut bad_role = peer_record("dev-2");
    bad_role.role = "admin".to_string();
    assert!(matches!(
        journal.peer_upsert(bad_role),
        Err(JournalError::InvalidRequest(_))
    ));
    let mut short_key = peer_record("dev-3");
    short_key.public_key = vec![0u8; 31];
    assert!(matches!(
        journal.peer_upsert(short_key),
        Err(JournalError::InvalidRequest(_))
    ));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audit_is_append_only_against_delete_and_update() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .audit_append(audit_record("dev-1", "Shutdown"))
        .expect("append");
    journal.flush().expect("flush");
    let conn = Connection::open(&path).expect("raw");
    let delete = conn.execute("DELETE FROM audit", []);
    assert!(delete.is_err(), "DELETE must fail on the audit table");
    assert!(
        delete
            .expect_err("delete error")
            .to_string()
            .contains("append-only"),
        "the trigger's message must name the reason"
    );
    let update = conn.execute("UPDATE audit SET outcome = 'ok'", []);
    assert!(update.is_err(), "UPDATE must fail on the audit table");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM audit", [], |row| row.get(0))
        .expect("count");
    assert_eq!(rows, 1);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audit_sweep_removes_only_rows_past_the_floor_and_restores_the_triggers() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let now = now_ms() as i64;
    journal
        .audit_append_at(audit_record("dev-1", "Ping"), now)
        .expect("fresh");
    let day = 24 * 60 * 60 * 1000;
    journal
        .audit_append_at(
            audit_record("dev-1", "Ping"),
            now - (journal_schema::AUDIT_FLOOR_DAYS + 1) * day,
        )
        .expect("aged");
    let sweep = journal.audit_sweep().expect("sweep");
    assert_eq!(sweep.deleted_by_age, 1);
    assert_eq!(sweep.deleted_by_cap, 0);

    let conn = Connection::open(&path).expect("raw");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM audit", [], |row| row.get(0))
        .expect("count");
    assert_eq!(rows, 1, "the fresh row must survive the sweep");
    for trigger in ["audit_no_delete", "audit_no_update"] {
        let present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                [trigger],
                |row| row.get(0),
            )
            .expect("trigger");
        assert_eq!(present, 1, "{trigger} must exist again after the sweep");
    }
    assert!(conn.execute("DELETE FROM audit", []).is_err());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audit_sweep_caps_rows_per_device_and_leaves_other_devices_alone() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal.flush().expect("flush");
    let at = now_ms() as i64;
    {
        let conn = Connection::open(&path).expect("raw");
        let tx = conn.unchecked_transaction().expect("tx");
        {
            let mut statement = tx
                .prepare(
                    "INSERT INTO audit (at, device_id, role, action, outcome)
                         VALUES (?1, ?2, 'daemon', 'Ping', 'ok')",
                )
                .expect("prepare");
            for index in 0..20_050i64 {
                statement
                    .execute(params![at + index, "dev-a"])
                    .expect("row");
            }
            statement
                .execute(params![at, "dev-b"])
                .expect("other device");
        }
        tx.commit().expect("commit");
    }
    let sweep = journal.audit_sweep().expect("sweep");
    assert_eq!(sweep.deleted_by_cap, 50, "20050 rows minus the 20000 cap");
    assert_eq!(sweep.deleted_by_age, 0, "all rows are fresh");

    let conn = Connection::open(&path).expect("raw");
    let capped: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audit WHERE device_id = 'dev-a'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(capped, 20_000);
    let other: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audit WHERE device_id = 'dev-b'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(other, 1, "another device's rows are untouched");
    let oldest: i64 = conn
        .query_row(
            "SELECT MIN(id) FROM audit WHERE device_id = 'dev-a'",
            [],
            |row| row.get(0),
        )
        .expect("min");
    assert_eq!(oldest, 51, "the oldest 50 rows are the ones removed");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reopening_recreates_a_missing_audit_trigger() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal.flush().expect("flush");
    drop(journal);
    {
        let conn = Connection::open(&path).expect("raw");
        conn.execute_batch("DROP TRIGGER IF EXISTS audit_no_delete;")
            .expect("drop trigger");
    }
    let journal = Journal::open(&path).expect("reopen");
    let conn = Connection::open(&path).expect("raw");
    let present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = 'audit_no_delete'",
                [],
                |row| row.get(0),
            )
            .expect("trigger");
    assert_eq!(present, 1);
    journal
        .audit_append(audit_record("dev-1", "Ping"))
        .expect("append");
    journal.flush().expect("flush");
    assert!(
        conn.execute("DELETE FROM audit", []).is_err(),
        "the recreated trigger must protect a non-empty table"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn session_retention_does_not_touch_audit_rows() {
    let (dir, path) = tmp_journal();
    let limits = JournalLimits {
        max_age_ms: 0,
        ..JournalLimits::default()
    };
    let journal = Journal::open_with_limits(&path, limits).expect("open");
    journal
        .audit_append(audit_record("dev-1", "SessionSend"))
        .expect("append");
    journal
        .upsert_blocking(sample_session("s.retention"))
        .expect("session");
    journal
        .append_blocking(output_record("s.retention", 1, 1, b"output"))
        .expect("output");
    journal.flush().expect("flush");
    let conn = Connection::open(&path).expect("raw");
    let audit_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM audit", [], |row| row.get(0))
        .expect("audit count");
    assert_eq!(audit_rows, 1, "retention never sweeps the audit table");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The tri-state ratchet, at the write it lives in (R2b): a row's
/// `unattended_state` may move up the order `no < unknown < yes` and never
/// down, whatever a later upsert carries — a resume rebuilt from an older
/// row, or an ordinary end marker, must not walk a birth fact backwards.
/// The boolean column beside it is the yes-absorbing term and ratchets the
/// same way, so a `yes` birth never loses its old-style flag either.
#[test]
fn the_unattended_state_ratchet_never_walks_a_row_back_down() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("journal");

    // `yes`, then a later write that says `no`.
    let mut yes = sample_session("s.ratchet.yes");
    yes.unattended_state = UnattendedState::Yes;
    journal.upsert_blocking(yes).expect("store yes");
    let mut downgraded = sample_session("s.ratchet.yes");
    downgraded.unattended_state = UnattendedState::No;
    journal.upsert_blocking(downgraded).expect("rewrite as no");

    // `unknown`, then a later write that says `no`.
    let mut unknown = sample_session("s.ratchet.unknown");
    unknown.unattended_state = UnattendedState::Unknown;
    journal.upsert_blocking(unknown).expect("store unknown");
    let mut lowered = sample_session("s.ratchet.unknown");
    lowered.unattended_state = UnattendedState::No;
    journal.upsert_blocking(lowered).expect("rewrite as no");

    journal.flush().expect("flush");
    let rows = journal.list().expect("list");
    let yes_row = rows
        .iter()
        .find(|row| row.id == "s.ratchet.yes")
        .expect("yes row");
    assert_eq!(
        yes_row.unattended_state,
        UnattendedState::Yes,
        "a later `no` write must not walk a `yes` birth backwards"
    );
    let unknown_row = rows
        .iter()
        .find(|row| row.id == "s.ratchet.unknown")
        .expect("unknown row");
    assert_eq!(
        unknown_row.unattended_state,
        UnattendedState::Unknown,
        "a later `no` write must not walk an `unknown` row down to `no`"
    );
    journal.shutdown();

    // The raw columns: the tri-state held, and the boolean column — the
    // yes-absorbing term — kept the `yes` birth's flag too.
    let conn = Connection::open(&path).expect("raw");
    let (state_rank, boolean): (i64, i64) = conn
        .query_row(
            "SELECT unattended_state, unattended FROM sessions WHERE id = 's.ratchet.yes'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("yes row columns");
    assert_eq!(state_rank, 2, "the tri-state stayed at yes");
    assert_eq!(boolean, 1, "the boolean column kept the yes birth");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A birth aimed at an id the journal already holds is refused, and the
/// held row survives untouched: the refusal replaces the merge that made
/// one row describe two sessions while neither's frames could be written.
#[test]
fn creating_a_session_on_a_held_id_is_refused_and_the_held_row_survives() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let mut held = sample_session("s.process-1234.00000001");
    held.kind = SessionKind::Acp;
    held.created_at_ms = 1_000;
    held.provider = Some("claude".to_string());
    journal.create_session(held).expect("the first birth lands");
    for seq in 1..=2 {
        journal
            .append_blocking(output_record("s.process-1234.00000001", 1, seq, b"frame"))
            .expect("event lands");
    }

    let mut second = sample_session("s.process-1234.00000001");
    second.kind = SessionKind::Terminal;
    second.provider = Some("fieldtest-grok".to_string());
    second.created_at_ms = 2_000;
    let refused = journal
        .create_session(second)
        .expect_err("a held id must refuse a second birth");
    assert!(
        matches!(refused, JournalError::SessionExists { .. }),
        "the refusal must name the collision: {refused:?}"
    );

    let listing = journal.list().expect("list");
    let rows: Vec<&SessionRecord> = listing
        .iter()
        .filter(|record| record.id == "s.process-1234.00000001")
        .collect();
    let [row] = rows.as_slice() else {
        panic!("exactly one row must hold the id, found {}", rows.len())
    };
    assert_eq!(row.kind, SessionKind::Acp, "kind stays the first session's");
    assert_eq!(row.created_at_ms, 1_000);
    assert_eq!(row.provider.as_deref(), Some("claude"));
    assert_eq!(
        row.last_seq, 2,
        "the refusal leaves the first session's stream in place"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The fix must not orphan anything recorded before it existed: an id in
/// the old counter-only shape still births, resolves, resumes and replays.
/// Green before the fix by design — it guards behaviour that must not
/// change; its teeth are proven by mutation in the report.
#[test]
fn ids_in_the_old_shape_still_resolve_resume_and_replay() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let old = sample_session("s.process-1234.00000001");
    journal
        .create_session(old)
        .expect("an old-shape id is still birthable");
    for seq in 1..=3 {
        journal
            .append_blocking(output_record(
                "s.process-1234.00000001",
                1,
                seq,
                b"old frame",
            ))
            .expect("event lands");
    }
    let found = journal
        .list()
        .expect("list")
        .into_iter()
        .find(|record| record.id == "s.process-1234.00000001");
    assert!(found.is_some(), "the old-shape row must resolve");
    // Replay derives view events from raw frames, so the count is not
    // ours to pin; the appended frames themselves must come back.
    let replay = journal.replay("s.process-1234.00000001").expect("replay");
    for seq in [1, 2, 3] {
        assert!(
            replay.event_seqs.iter().any(|&(_, s)| s == seq),
            "appended frame {seq} must still replay: {:?}",
            replay.event_seqs
        );
    }
    // The journal half of resume: the generation bumps on the held row.
    journal
        .start_generation("s.process-1234.00000001", 2)
        .expect("resume generation");
    // The attachment half: the id is still a folder name the store serves.
    let store = crate::attachment_store::AttachmentStore::new(&dir);
    assert!(
        store.session("s.process-1234.00000001").is_some(),
        "the old-shape id must still resolve for attachments"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A resume keeps the session id and every earlier generation's events,
/// so a transcript read must return the whole history, ordered by
/// (generation, seq) — the order the events_session index serves.
#[test]
fn replay_returns_whole_history_across_generations() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let id = "s.cross.gen.replay";
    journal.create_session(sample_session(id)).expect("birth");
    let user_before = SessionEvent::AgentUserMessage {
        message_id: Some("m1".into()),
        text: "gen-1 user".into(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Unknown,
    };
    let answer_before = SessionEvent::AgentMessage {
        message_id: Some("m2".into()),
        text: "gen-1 answer".into(),
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    let answer_after = SessionEvent::AgentMessage {
        message_id: Some("m3".into()),
        text: "after resume".into(),
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    journal
        .append_blocking(agent_report_record(id, 1, 1, &user_before).unwrap())
        .expect("gen-1 user row");
    journal
        .append_blocking(agent_report_record(id, 1, 2, &answer_before).unwrap())
        .expect("gen-1 answer row");
    journal.start_generation(id, 2).expect("resume generation");
    journal
        .append_blocking(agent_report_record(id, 2, 1, &answer_after).unwrap())
        .expect("gen-2 answer row");
    let replay = journal.replay(id).expect("replay");
    let transcript: Vec<String> = replay
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentUserMessage { text, .. } => Some(text.clone()),
            SessionEvent::AgentMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        transcript,
        vec!["gen-1 user", "gen-1 answer", "after resume"],
        "the transcript must span the resume seam in journal order: {transcript:?}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The session's exit code is the current generation's end. An Exit row
/// journaled by an earlier generation predates the resume and must not
/// be reported as the session's exit — the sessions row's own
/// `exit_code` is the authority when the current generation left no
/// Exit row of its own.
#[test]
fn a_previous_generations_exit_row_does_not_speak_for_the_session() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let id = "s.cross.gen.exit";
    journal.create_session(sample_session(id)).expect("birth");
    let frame = SessionEvent::AgentUserMessage {
        message_id: Some("m1".into()),
        text: "gen-1 user".into(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Unknown,
    };
    journal
        .append_blocking(agent_report_record(id, 1, 1, &frame).unwrap())
        .expect("gen-1 row");
    // Generation 1 ended with an observed exit, code 7.
    journal
        .append_blocking(EventRecord {
            session_id: id.to_string(),
            generation: 1,
            seq: 2,
            kind: EventKind::Exit,
            ts_ms: now_ms(),
            payload: 7u32.to_le_bytes().to_vec(),
        })
        .expect("gen-1 exit row");
    journal.start_generation(id, 2).expect("resume generation");
    let frame_after = SessionEvent::AgentMessage {
        message_id: Some("m2".into()),
        text: "after resume".into(),
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    journal
        .append_blocking(agent_report_record(id, 2, 1, &frame_after).unwrap())
        .expect("gen-2 row");
    // The current generation was reaped without leaving an Exit row of
    // its own — the `try_mark_ended` command was dropped on a full
    // queue while the wait observed code 0 — so the row says
    // live + reaped + exit_code 0.
    journal.mark_reaped(id, Some(0)).expect("mark reaped");
    let replay = journal.replay(id).expect("replay");
    let exit_codes: Vec<Option<u32>> = replay
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Exit { code } => Some(*code),
            _ => None,
        })
        .collect();
    assert_eq!(
        exit_codes,
        vec![Some(0)],
        "generation 1's exit row must not be reported as the session's exit: {exit_codes:?}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The live-agent paging read must serve the same whole-history contract:
/// rows from generations up to the expected one, in (generation, seq)
/// order, each record stamped with its own generation.
#[test]
fn agent_page_spans_generations_in_journal_order() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let id = "s.cross.gen.page";
    journal.create_session(sample_session(id)).expect("birth");
    let user_before = SessionEvent::AgentUserMessage {
        message_id: Some("m1".into()),
        text: "gen-1 user".into(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Unknown,
    };
    let answer_before = SessionEvent::AgentMessage {
        message_id: Some("m2".into()),
        text: "gen-1 answer".into(),
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    let answer_after = SessionEvent::AgentMessage {
        message_id: Some("m3".into()),
        text: "after resume".into(),
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    journal
        .append_blocking(agent_report_record(id, 1, 1, &user_before).unwrap())
        .expect("gen-1 user row");
    journal
        .append_blocking(agent_report_record(id, 1, 2, &answer_before).unwrap())
        .expect("gen-1 answer row");
    journal.start_generation(id, 2).expect("resume generation");
    journal
        .append_blocking(agent_report_record(id, 2, 1, &answer_after).unwrap())
        .expect("gen-2 answer row");
    // Attaching to generation 2 with a from_seq of 0 replays the whole
    // history: both generations, oldest first.
    let page = journal.replay_agent_page(id, 2, 0, 0, 1, 10).expect("page");
    let rows: Vec<(u64, u64)> = page
        .records
        .iter()
        .map(|record| (record.generation, record.seq))
        .collect();
    assert_eq!(
        rows,
        vec![(1, 1), (1, 2), (2, 1)],
        "the page must span the resume seam in (generation, seq) order: {rows:?}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The page bounds are bound as i64, and a u64::MAX from-seq — the
/// nothing-owed sentinel a stop-tail attach carries — would wrap
/// negative and re-serve rows the reader asked to skip. The bind must
/// clamp structurally, not rely on a caller's short-circuit upstream.
#[test]
fn the_nothing_owed_sentinel_cannot_widen_a_page_range() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let id = "s.cross.gen.sentinel";
    journal.create_session(sample_session(id)).expect("birth");
    let user_before = SessionEvent::AgentUserMessage {
        message_id: Some("m1".into()),
        text: "gen-1 user".into(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Unknown,
    };
    let answer_after = SessionEvent::AgentMessage {
        message_id: Some("m2".into()),
        text: "after resume".into(),
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    journal
        .append_blocking(agent_report_record(id, 1, 1, &user_before).unwrap())
        .expect("gen-1 user row");
    journal.start_generation(id, 2).expect("resume generation");
    journal
        .append_blocking(agent_report_record(id, 2, 1, &answer_after).unwrap())
        .expect("gen-2 answer row");
    // The stop-tail shape: from-seq at the sentinel, through-seq sane.
    let page = journal
        .replay_agent_page(id, 2, 2, u64::MAX, 1, 10)
        .expect("page");
    assert!(
        page.records.is_empty(),
        "a nothing-owed from-seq must narrow the page to nothing, not widen it: {:?}",
        page.records
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// M1: the held-id refusal must rest on the constraint code, not the prose.
/// Both errors below come from SQLite itself — no hand-made error value.
/// A real non-PK failure whose message names the column must not read as
/// held; a real PK duplicate must.
#[test]
fn session_id_taken_rests_on_the_constraint_code_not_the_prose() {
    let conn = Connection::open_in_memory().expect("in-memory");
    conn.execute_batch(
        "CREATE TABLE sessions(id TEXT PRIMARY KEY);
             CREATE TRIGGER trg BEFORE INSERT ON sessions
             BEGIN SELECT RAISE(ABORT, 'sessions.id lookalike'); END;",
    )
    .expect("scratch sessions table with a prose-matching trigger");
    let trigger_err = conn
        .execute("INSERT INTO sessions(id) VALUES ('x')", [])
        .expect_err("the trigger fires");
    assert!(
        !session_id_taken(&trigger_err),
        "prose alone must not classify a held id: {trigger_err:?}"
    );
    conn.execute_batch("DROP TRIGGER trg;")
        .expect("drop trigger");
    conn.execute("INSERT INTO sessions(id) VALUES ('x')", [])
        .expect("first insert lands");
    let pk_err = conn
        .execute("INSERT INTO sessions(id) VALUES ('x')", [])
        .expect_err("duplicate primary key");
    assert!(
        session_id_taken(&pk_err),
        "a real primary-key duplicate must read as held: {pk_err:?}"
    );
    assert_eq!(
        pk_err.sqlite_error().map(|error| error.extended_code),
        Some(rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY),
        "the positive half really is a primary-key violation"
    );
    assert_ne!(
        trigger_err.sqlite_error().map(|error| error.extended_code),
        Some(rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY),
        "the negative half really is not"
    );
}

/// J3: a birth changes the roster, so it must bump the revision the
/// roster cache is keyed on. The older Upsert arm does; the new
/// CreateSession arm currently does not.
#[test]
fn create_session_bumps_roster_revision_on_success_and_not_on_refusal() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let before = journal.session_set_revision();
    journal
        .create_session(sample_session("s.j3-rev-1"))
        .expect("birth lands");
    let after = journal.session_set_revision();
    assert!(
        after > before,
        "birth must bump roster revision: before={before} after={after}"
    );
    let before_refusal = journal.session_set_revision();
    let refused = journal
        .create_session(sample_session("s.j3-rev-1"))
        .expect_err("held id refuses");
    assert!(
        matches!(refused, JournalError::SessionExists { .. }),
        "refusal must name collision: {refused:?}"
    );
    let after_refusal = journal.session_set_revision();
    assert_eq!(
        after_refusal, before_refusal,
        "a refusal changes no roster, so it must not bump"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The handle column's writers, in sequence on one row — these are
/// sequential rules, exercised one after another; nothing here claims an
/// interleaving. The announce persists a changed handle and bumps the
/// roster revision; a same-value announce changes nothing and costs no
/// rebuild; the disown mark records a refusal beside the handle WITHOUT
/// destroying it, only while the row still carries the handle it names;
/// and an announce of a DIFFERENT handle clears the mark, because a
/// refusal about a retired handle must not silence the fresh one.
#[test]
fn peer_handle_announce_and_disown_mark_rules_in_sequence() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let mut record = sample_session("s.peer.handle");
    // The verdict under test needs a family the resume road admits.
    record.kind = SessionKind::Acp;
    record.provider = Some("stub".to_string());
    journal.create_session(record).expect("birth lands");
    let before = journal.session_set_revision();

    // First announce: NULL -> value is a change, and it bumps.
    journal
        .set_peer_session_id("s.peer.handle", "peer-1")
        .expect("first announce lands");
    assert!(
        journal.session_set_revision() > before,
        "a changed handle must bump the roster revision"
    );
    let after_set = journal.session_set_revision();

    // The announce-time writers re-persist the same id on every frame.
    journal
        .set_peer_session_id("s.peer.handle", "peer-1")
        .expect("same-value announce lands");
    assert_eq!(
        journal.session_set_revision(),
        after_set,
        "an unchanged write changed nothing the roster renders"
    );

    // The mark names the handle that failed to load. A different
    // expected handle matches nothing and records nothing.
    journal
        .mark_peer_session_disowned("s.peer.handle", "peer-other")
        .expect("a mark about another handle is a no-op, not an error");
    assert_eq!(
        journal.session_set_revision(),
        after_set,
        "a no-op mark must not bump"
    );

    // The matching mark lands — beside the handle, never in its place.
    journal
        .mark_peer_session_disowned("s.peer.handle", "peer-1")
        .expect("the matching mark lands");
    assert!(
        journal.session_set_revision() > after_set,
        "a real mark must bump the roster revision"
    );
    let row = journal
        .list()
        .expect("list")
        .into_iter()
        .find(|record| record.id == "s.peer.handle")
        .expect("row");
    assert_eq!(
        row.peer_session_id.as_deref(),
        Some("peer-1"),
        "the refused handle is never destroyed"
    );
    assert_eq!(
        row.disowned_peer_session_id.as_deref(),
        Some("peer-1"),
        "the refusal is recorded"
    );
    assert!(
        !row.to_session().resumable,
        "a row whose handle was refused does not offer the refused resume"
    );

    // Idempotent: the same refusal twice writes once.
    let after_mark = journal.session_set_revision();
    journal
        .mark_peer_session_disowned("s.peer.handle", "peer-1")
        .expect("the repeat mark is a no-op");
    assert_eq!(
        journal.session_set_revision(),
        after_mark,
        "a repeated mark changed nothing"
    );

    // A fresh handle announced after the refusal clears it — the refusal
    // was about a handle that no longer applies — and the offer returns
    // on its own. This is what makes the mechanism reversible.
    journal
        .set_peer_session_id("s.peer.handle", "peer-2")
        .expect("the fresh announce lands");
    let row = journal
        .list()
        .expect("list")
        .into_iter()
        .find(|record| record.id == "s.peer.handle")
        .expect("row");
    assert_eq!(
        row.disowned_peer_session_id, None,
        "the mark must not silence a handle it is not about"
    );
    assert!(
        row.to_session().resumable,
        "a fresh handle restores the offer by itself"
    );

    // A missing row stays the error it has always been.
    let missing = journal
        .set_peer_session_id("s.peer.absent", "peer-1")
        .expect_err("a missing row is an error");
    assert!(
        matches!(missing, JournalError::SessionNotFound),
        "missing row must read as SessionNotFound: {missing:?}"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// J1: a birth whose caller is gone must not leave a row behind.
///
/// A timed-out `create_session` (slow writer, full queue) leaves exactly
/// this state: the command is queued but the reply receiver is dropped.
/// The writer currently INSERTs anyway and the reply goes nowhere,
/// leaving a Live row with no owner that consumes the id for good.
#[test]
fn abandoned_birth_leaves_no_row() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let record = sample_session("s.j1-ghost-1");
    let (tx_reply, rx_reply) = mpsc::channel();
    drop(rx_reply);
    journal.reserve_slot();
    journal
        .tx
        .try_send(JournalCmd::CreateSession {
            record,
            reply: tx_reply,
        })
        .expect("enqueue abandoned birth");
    std::thread::sleep(Duration::from_millis(500));
    let listing = journal.list().expect("list");
    assert!(
        !listing.iter().any(|row| row.id == "s.j1-ghost-1"),
        "an abandoned birth must not leave a row behind"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

//! Claude resume against the stub CLI: the daemon seam, not the provider.
//!
//! A killed daemon takes the stub with it; the reboot lists the row as
//! `Recovered`; resume must respawn the CLI with `--resume <peer>` onto the
//! SAME row (same id, generation + 1), never a second session. The stub has
//! no memory — context survival is the live battery's job
//! (`claude_resume_live.rs`).

#![cfg(windows)]

#[path = "claude_common/mod.rs"]
mod common;

use std::time::Duration;

use devboule_protocol::{
    Persistence, PersistenceKind, ResumeResult, SessionEvent, SessionKind, SessionState,
};
use rusqlite::Connection;

fn is_finished(events: &[SessionEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, SessionEvent::AgentFinished { .. }))
}

fn is_manifest(events: &[SessionEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, SessionEvent::SessionManifest { .. }))
}

fn session_rows(journal: &std::path::Path, id: &str) -> i64 {
    let connection = Connection::open(journal).expect("open journal");
    connection
        .query_row("SELECT COUNT(*) FROM sessions WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .expect("count session rows")
}

fn wait_peer(client: &devboule_daemon::DaemonClient, id: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let peer = client
            .sessions_list()
            .expect("list sessions")
            .into_iter()
            .find(|listed| listed.id == id)
            .and_then(|listed| listed.peer_session_id);
        if let Some(peer) = peer {
            return peer;
        }
        if std::time::Instant::now() > deadline {
            panic!("stub never published its peer session id");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The stub records its argv with a truncating `fs::write` on its own
/// schedule after the daemon has spawned it
/// (`devboule_claude_stub.rs:183`), and swallows write errors — so one
/// unsynchronised read can return the previous spawn's argv, a partial line,
/// or nothing. Wait for the property the assertion needs, and on timeout
/// fail with what the file actually held.
fn wait_argv_contains(argv_file: &std::path::Path, needle: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let contents = std::fs::read_to_string(argv_file).unwrap_or_default();
        if contents.contains(needle) {
            return contents;
        }
        if std::time::Instant::now() > deadline {
            panic!("the stub's argv never contained {needle:?}; argv file contents: {contents:?}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn claude_resume_after_daemon_death_reuses_the_row() {
    let _test_lock = common::lock_tests();
    let observation =
        std::env::temp_dir().join(format!("devboule-claude-resume-{}-1", std::process::id()));
    std::fs::create_dir_all(&observation).expect("observation dir");
    let argv_file = observation.join("argv.txt");
    let console_file = observation.join("console.txt");
    let home = observation.join("home");
    std::fs::create_dir_all(&home).expect("fake home");
    let _env = common::use_stub_cli(&argv_file, &console_file, &home);

    let mut harness = common::Harness::spawn();
    let client = std::sync::Arc::new(harness.client());
    let session = client
        .session_create(None, SessionKind::Claude, None)
        .expect("create Claude session");
    assert_eq!(session.kind, SessionKind::Claude);
    assert_eq!(session.provider.as_deref(), Some("claude"));

    let (events, handler) = common::collect_events();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach Claude session");
    common::wait_for(&events, Duration::from_secs(15), "manifest", is_manifest);
    let peer = wait_peer(&client, &session.id);

    client
        .session_send(&session.id, "PRIME")
        .expect("prompt before daemon death");
    common::wait_for(
        &events,
        Duration::from_secs(15),
        "first answer",
        is_finished,
    );
    assert!(
        std::fs::read_to_string(&console_file)
            .expect("stub console")
            .contains("PRIME"),
        "the prompt reached the stub child"
    );

    // The daemon dies with its children; the reboot recovers the transcript.
    harness.restart();
    let client = harness.client_named("restarted");
    let recovered = client
        .sessions_list()
        .expect("list sessions")
        .into_iter()
        .find(|listed| listed.id == session.id)
        .expect("recovered session missing");
    assert!(
        matches!(recovered.state, SessionState::Recovered { .. }),
        "a killed daemon leaves a Recovered row, not a live one"
    );
    assert_eq!(recovered.peer_session_id.as_deref(), Some(peer.as_str()));
    assert!(
        recovered.resumable,
        "the wire carries the daemon's own verdict for a dead Claude row"
    );

    let resumed = client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Claude {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect("resume recovered Claude session");
    let resumed = match resumed {
        ResumeResult::Resumed { session } => *session,
        ResumeResult::NotSupported => panic!("Claude resume answered NotSupported"),
        ResumeResult::Failed { message } => panic!("Claude resume failed: {message}"),
    };
    assert_eq!(resumed.id, session.id, "resume reuses the row");
    assert!(
        matches!(resumed.state, SessionState::Live { generation: 2 }),
        "resume continues under a new generation: {:?}",
        resumed.state
    );
    assert_eq!(
        session_rows(&harness.journal_path(), &session.id),
        1,
        "a resumed row is not a second session in the journal"
    );

    let (events, handler) = common::collect_events();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach resumed Claude session");
    client
        .session_send(&session.id, "FOLLOW-UP")
        .expect("prompt after resume");
    common::wait_for(
        &events,
        Duration::from_secs(15),
        "second answer",
        is_finished,
    );
    let console = std::fs::read_to_string(&console_file).expect("stub console");
    assert!(console.contains("FOLLOW-UP"), "the rebound child answers");
    let argv = wait_argv_contains(&argv_file, peer.as_str());
    assert!(
        argv.lines().any(|line| line == "--resume"),
        "the respawn carried the provider's resume flag: {argv}"
    );
    client
        .session_close(&session.id)
        .expect("close Claude session");
    let _ = std::fs::remove_dir_all(&observation);
}

#[test]
fn claude_resume_accepts_the_old_acp_tag() {
    // The tag names the family that wrote the row; it never admits. An app
    // that still sends the pre-Claude tag resumes the same row — the journal
    // decides, through `Provider::resumable()`.
    let _test_lock = common::lock_tests();
    let observation =
        std::env::temp_dir().join(format!("devboule-claude-resume-{}-2", std::process::id()));
    std::fs::create_dir_all(&observation).expect("observation dir");
    let argv_file = observation.join("argv.txt");
    let console_file = observation.join("console.txt");
    let home = observation.join("home");
    std::fs::create_dir_all(&home).expect("fake home");
    let _env = common::use_stub_cli(&argv_file, &console_file, &home);

    let mut harness = common::Harness::spawn();
    let client = harness.client();
    let session = client
        .session_create(None, SessionKind::Claude, None)
        .expect("create Claude session");
    let (events, handler) = common::collect_events();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach Claude session");
    common::wait_for(&events, Duration::from_secs(15), "manifest", is_manifest);
    let peer = wait_peer(&client, &session.id);

    harness.restart();
    let client = harness.client_named("restarted");
    let resumed = client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Acp {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect("resume with the old tag");
    match resumed {
        ResumeResult::Resumed { session } => {
            assert!(matches!(
                session.state,
                SessionState::Live { generation: 2 }
            ));
        }
        ResumeResult::NotSupported => panic!("old tag answered NotSupported"),
        ResumeResult::Failed { message } => panic!("old tag resume failed: {message}"),
    }
    // The whole point of this test: the resumed child was spawned with the
    // conversation id, and the argv file is the only record of that.
    wait_argv_contains(&argv_file, peer.as_str());
    client
        .session_close(&session.id)
        .expect("close Claude session");
    let _ = std::fs::remove_dir_all(&observation);
}

#[test]
fn claude_resume_refuses_a_deleted_history_by_name() {
    // The human deleted the provider's file (or the CLI rotated it): the
    // resume refuses naming the conversation, before any process exists.
    let _test_lock = common::lock_tests();
    let observation =
        std::env::temp_dir().join(format!("devboule-claude-resume-{}-3", std::process::id()));
    std::fs::create_dir_all(&observation).expect("observation dir");
    let argv_file = observation.join("argv.txt");
    let console_file = observation.join("console.txt");
    let home = observation.join("home");
    std::fs::create_dir_all(&home).expect("fake home");
    let _env = common::use_stub_cli(&argv_file, &console_file, &home);

    let mut harness = common::Harness::spawn();
    let client = harness.client();
    let session = client
        .session_create(None, SessionKind::Claude, None)
        .expect("create Claude session");
    let (events, handler) = common::collect_events();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach Claude session");
    common::wait_for(&events, Duration::from_secs(15), "manifest", is_manifest);
    let peer = wait_peer(&client, &session.id);

    harness.restart();
    let client = harness.client_named("restarted");
    // The stub filed its fake conversation under the fake home; deleting it
    // is the human deleting `~/.claude/projects/<slug>/<peer>.jsonl`.
    let projects = home.join(".claude").join("projects");
    let mut deleted = false;
    for slug in std::fs::read_dir(&projects).expect("projects dir") {
        let candidate = slug.expect("slug dir").path().join(format!("{peer}.jsonl"));
        if candidate.is_file() {
            std::fs::remove_file(&candidate).expect("delete history");
            deleted = true;
        }
    }
    assert!(deleted, "the stub must have filed a history to delete");

    let error = client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Claude {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect_err("a deleted history must refuse");
    let message = error.to_string();
    assert!(
        message.contains(peer.as_str()),
        "the refusal names the conversation: {message}"
    );
    let _ = std::fs::remove_dir_all(&observation);
}

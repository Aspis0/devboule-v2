//! The proof stage 1 stands on: the PROVIDER's conversation continues.
//!
//! A real `claude` CLI, a primed context word, a killed daemon, a resume, a
//! follow-up that only the earlier context can answer. The stub battery
//! proves the daemon seam; this proves the other side of it.
//!
//! Ignored by default: it spends two small model calls and minutes of wall
//! time, and needs an authenticated CLI. Run it explicitly:
//! `cargo test -p devboule-daemon --test claude_resume_live -- --ignored --nocapture`

#![cfg(windows)]

#[path = "claude_common/mod.rs"]
mod common;

use std::time::{Duration, Instant};

use devboule_protocol::{
    Persistence, PersistenceKind, ResumeResult, SessionEvent, SessionKind, SessionState,
};
use rusqlite::Connection;

/// The slug rule the daemon's pre-resume check implements, restated here so
/// the test fails if the two disagree about where the CLI keeps a cwd.
fn slug(cwd: &std::path::Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|cell| {
            if cell.is_ascii_alphanumeric() || cell == '-' {
                cell
            } else {
                '-'
            }
        })
        .collect()
}

fn agent_texts(events: &[SessionEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The streamed word arrives in pieces (`B`, `L`, `UEBIRD`); the answer is
/// there once the pieces spell it.
fn says_bluebird(events: &[SessionEvent]) -> bool {
    agent_texts(events)
        .chars()
        .filter(|cell| !cell.is_whitespace())
        .collect::<String>()
        .to_uppercase()
        .contains("BLUEBIRD")
}

#[test]
#[ignore = "two real model calls; needs an authenticated claude CLI"]
fn claude_resume_continues_the_providers_conversation() {
    let _test_lock = common::lock_tests();
    std::env::remove_var("DEVBOULE_CLAUDE_COMMAND");

    let mut harness = common::Harness::spawn();
    let client = std::sync::Arc::new(harness.client());
    let session = client
        .session_create(None, SessionKind::Claude, None)
        .expect("create Claude session");

    let (events, handler) = common::collect_events();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach Claude session");
    client
        .session_send(
            &session.id,
            "Reply with exactly the word BLUEBIRD and nothing else.",
        )
        .expect("prime the conversation");
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        // Read under a short lock, decide outside it: a panic while holding
        // the guard would poison the reader thread behind it.
        let answered = says_bluebird(&events.lock().expect("events lock"));
        if answered {
            break;
        }
        if Instant::now() > deadline {
            let texts = agent_texts(&events.lock().unwrap_or_else(|error| error.into_inner()));
            panic!("the prime was never answered: {texts:?}");
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    eprintln!(
        "prime answered: {:?}",
        agent_texts(&events.lock().expect("events lock"))
    );

    // The provider's own file, named by the peer id the daemon stored.
    let peer = client
        .sessions_list()
        .expect("list sessions")
        .into_iter()
        .find(|listed| listed.id == session.id)
        .and_then(|listed| listed.peer_session_id)
        .expect("peer session id persisted");
    let home = std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .expect("home directory");
    // The daemon resolves the cwd from the workspace; the test restates the
    // temp-dir case directly: sessions created without a workspace start in
    // the daemon's own working directory.
    let daemon_cwd = std::env::current_dir().expect("daemon working directory");
    let history = home
        .join(".claude")
        .join("projects")
        .join(slug(&daemon_cwd))
        .join(format!("{peer}.jsonl"));
    assert!(
        history.is_file(),
        "the provider kept its conversation where the slug rule says: {}",
        history.to_string_lossy()
    );

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
        "a killed daemon leaves a Recovered row"
    );
    assert!(recovered.resumable, "the dead Claude row offers resume");

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
    match resumed {
        ResumeResult::Resumed { session } => {
            assert!(matches!(
                session.state,
                SessionState::Live { generation: 2 }
            ));
        }
        ResumeResult::NotSupported => panic!("Claude resume answered NotSupported"),
        ResumeResult::Failed { message } => panic!("Claude resume failed: {message}"),
    }

    let (events, handler) = common::collect_events();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach resumed Claude session");
    client
        .session_send(
            &session.id,
            "What exact word did I ask you to reply with in my first message? Reply with just that word and nothing else.",
        )
        .expect("follow-up after resume");
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let answered = says_bluebird(&events.lock().expect("events lock"));
        if answered {
            break;
        }
        if Instant::now() > deadline {
            let texts = agent_texts(&events.lock().unwrap_or_else(|error| error.into_inner()));
            panic!("the context did not survive the resume: {texts:?}");
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    eprintln!(
        "follow-up answered after resume: {:?}",
        agent_texts(&events.lock().expect("events lock"))
    );

    let connection = Connection::open(harness.journal_path()).expect("open journal");
    let rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1",
            [&session.id],
            |row| row.get(0),
        )
        .expect("count session rows");
    assert_eq!(rows, 1, "a resumed row is not a second session");
    client
        .session_close(&session.id)
        .expect("close Claude session");
    // Best-effort: our two-message probe session does not belong in the
    // human's conversation picker.
    let _ = std::fs::remove_file(&history);
}

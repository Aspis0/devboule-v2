//! The proof stage 1 stands on: the PROVIDER's conversation continues.
//!
//! A real `claude` CLI, a primed context word, a killed daemon, a resume, a
//! follow-up that only the earlier context can answer. The stub battery
//! proves the daemon seam; this proves the other side of it.
//!
//! The follow-up asks for the primed word **reversed**, so the answer is
//! derived, not repeated: a replayed journal snapshot cannot contain it, and
//! neither can a provider that came back without its conversation. A second
//! session, with no context at all, is asked the same question as a negative
//! control — it must fail the same assertion, which is what makes the probe
//! discriminating.
//!
//! Ignored by default: it spends three small model calls and minutes of wall
//! time, and needs an authenticated CLI. Run it explicitly:
//! `cargo test -p devboule-daemon --test claude_resume_live -- --ignored --nocapture`

#![cfg(windows)]

#[path = "claude_common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use devboule_daemon::DaemonClient;
use devboule_protocol::{
    Persistence, PersistenceKind, ResumeResult, SessionEvent, SessionKind, SessionState,
};
use rusqlite::Connection;

/// The word the prime asks for.
const PRIME_WORD: &str = "BLUEBIRD";
/// Never spelled by either prompt: only a provider that still holds the
/// conversation can derive it, and no replay of our journal can contain it.
const REVERSED_WORD: &str = "DRIBEULB";
/// The follow-up, verbatim: the resumed session and the negative control are
/// asked the same question, so the two answers differ only by context.
const FOLLOW_UP: &str =
    "What exact word did I ask you to reply with in my first message? Reply with that word spelled backwards, and nothing else.";

/// The slug rule the daemon's pre-resume check implements, restated here so
/// the test fails if the two disagree about where the CLI keeps a cwd.
fn slug(cwd: &Path) -> String {
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

/// The streamed word arrives in pieces (`D`, `RIBEULB`); it is there once the
/// pieces spell it, whatever whitespace separates them.
fn spells(text: &str, word: &str) -> bool {
    text.chars()
        .filter(|cell| !cell.is_whitespace())
        .collect::<String>()
        .to_uppercase()
        .contains(word)
}

fn says(events: &[SessionEvent], word: &str) -> bool {
    spells(&agent_texts(events), word)
}

fn has_finished(events: &[SessionEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, SessionEvent::AgentFinished { .. }))
}

/// Wait until `done` holds over the collected events, and answer the agent
/// text seen at that moment. A timeout panics with what did arrive, so a
/// failing run says whether the provider answered something else — or nothing.
fn wait_for_events(
    events: &Mutex<Vec<SessionEvent>>,
    what: &str,
    mut done: impl FnMut(&[SessionEvent]) -> bool,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        {
            // Decide on a snapshot taken under the lock, released before the
            // sleep: a panic while holding the guard would poison the reader
            // thread parked behind it.
            let collected = events.lock().expect("events lock");
            if done(&collected) {
                return agent_texts(&collected);
            }
        }
        if Instant::now() > deadline {
            let collected = events.lock().unwrap_or_else(|error| error.into_inner());
            panic!("{what}: {:?}", agent_texts(&collected));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn peer_of(client: &DaemonClient, session_id: &str) -> String {
    client
        .sessions_list()
        .expect("list sessions")
        .into_iter()
        .find(|listed| listed.id == session_id)
        .and_then(|listed| listed.peer_session_id)
        .expect("peer session id persisted")
}

/// The provider's own file for one conversation, under the slug rule above.
fn history_path(home: &Path, cwd: &Path, peer: &str) -> PathBuf {
    home.join(".claude")
        .join("projects")
        .join(slug(cwd))
        .join(format!("{peer}.jsonl"))
}

#[test]
#[ignore = "three real model calls; needs an authenticated claude CLI"]
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
    let prime_text = wait_for_events(&events, "the prime was never answered", |events| {
        has_finished(events) && says(events, PRIME_WORD)
    });
    eprintln!("prime answered: {prime_text:?}");

    // The provider's own file, named by the peer id the daemon stored.
    let peer = peer_of(&client, &session.id);
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .expect("home directory");
    // The daemon resolves the cwd from the workspace; the test restates the
    // temp-dir case directly: sessions created without a workspace start in
    // the daemon's own working directory.
    let daemon_cwd = std::env::current_dir().expect("daemon working directory");
    let history = history_path(&home, &daemon_cwd, &peer);
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
    // The attach replays our own journal. Wait for the whole replayed prime
    // turn (its answer plus its end marker), then empty the collector: from
    // here the probe watches only what arrives after the replay. Without the
    // drain, the assertion below could be satisfied by the snapshot the
    // daemon rewinds — the false proof this test exists to rule out.
    let replayed = wait_for_events(&events, "the journal replay never landed", |events| {
        has_finished(events) && says(events, PRIME_WORD)
    });
    eprintln!("replayed after resume: {replayed:?}");
    events.lock().expect("events lock").clear();

    client
        .session_send(&session.id, FOLLOW_UP)
        .expect("follow-up after resume");
    let follow_up = wait_for_events(
        &events,
        "the context did not survive the resume",
        |events| says(events, REVERSED_WORD),
    );
    eprintln!("follow-up answered after resume: {follow_up:?}");

    // The negative control: the same derived question to a session with no
    // context. It must fail the assertion the resumed session just passed,
    // or the assertion proves nothing about context.
    let control = client
        .session_create(None, SessionKind::Claude, None)
        .expect("create the control session");
    let (control_events, control_handler) = common::collect_events();
    client
        .session_attach(&control.id, None, control_handler)
        .expect("attach the control session");
    client
        .session_send(&control.id, FOLLOW_UP)
        .expect("ask the control session");
    let control_text = wait_for_events(&control_events, "the control never answered", |events| {
        has_finished(events)
    });
    assert!(
        !spells(&control_text, REVERSED_WORD),
        "an empty-context agent must not produce the derived word; it answered {control_text:?}"
    );
    eprintln!("control answered without the context: {control_text:?}");

    let connection = Connection::open(harness.journal_path()).expect("open journal");
    let rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = ?1",
            [&session.id],
            |row| row.get(0),
        )
        .expect("count session rows");
    assert_eq!(rows, 1, "a resumed row is not a second session");
    let control_peer = peer_of(&client, &control.id);
    client
        .session_close(&session.id)
        .expect("close Claude session");
    client
        .session_close(&control.id)
        .expect("close the control session");
    // Best-effort: our probe sessions do not belong in the human's
    // conversation picker.
    let _ = std::fs::remove_file(&history);
    let _ = std::fs::remove_file(history_path(&home, &daemon_cwd, &control_peer));
}

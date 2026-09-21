//! The proof stage 2 stands on for Codex: the PROVIDER's thread continues.
//!
//! A real `codex app-server`, a primed context word, a killed daemon, a
//! `thread/resume`, a follow-up that only the earlier thread can answer. The
//! stub battery proves the daemon seam; this proves the other side of it.
//!
//! The follow-up asks for the primed word **reversed**, so the answer is
//! derived, not repeated: a replayed journal snapshot cannot contain it, and
//! neither can a provider that came back without its conversation. A second
//! session, with no context at all, is asked the same question as a negative
//! control — it must fail the same assertion, which is what makes the probe
//! discriminating.
//!
//! Ignored by default: it spends three small model calls and minutes of wall
//! time, and needs an authenticated Codex CLI. Run it explicitly:
//! `cargo test -p devboule-daemon --test codex_resume_live -- --ignored --nocapture`

#![cfg(windows)]

#[path = "claude_common/mod.rs"]
mod common;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use devboule_protocol::NoticeSeverity;
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

/// A warning notice is how Codex reports a turn that failed: the wait must
/// fail with that sentence instead of outliving the turn by ten minutes.
fn failure_notice(events: &[SessionEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        SessionEvent::SessionNotice { text, severity } if *severity == NoticeSeverity::Warning => {
            Some(text.clone())
        }
        _ => None,
    })
}

/// Wait until `done` holds over the collected events, and answer the agent
/// text seen at that moment. A timeout — or a failure notice — panics with
/// what did arrive, so a failing run says why.
fn wait_for_events(
    events: &Mutex<Vec<SessionEvent>>,
    what: &str,
    mut done: impl FnMut(&[SessionEvent]) -> bool,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let notice = {
            // Decide on a snapshot taken under the lock, released before the
            // sleep: a panic while holding the guard would poison the reader
            // thread parked behind it.
            let collected = events.lock().expect("events lock");
            if done(&collected) {
                return agent_texts(&collected);
            }
            failure_notice(&collected)
        };
        if let Some(text) = notice {
            panic!("{what}: the provider reported {text}");
        }
        if Instant::now() > deadline {
            let collected = events.lock().unwrap_or_else(|error| error.into_inner());
            panic!("{what}: {:?}", agent_texts(&collected));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[test]
#[ignore = "three real model calls; needs an authenticated codex CLI"]
fn codex_resume_continues_the_providers_thread() {
    let _test_lock = common::lock_tests();

    let mut harness = common::Harness::spawn();
    let client = harness.client();
    let session = client
        .session_create(None, SessionKind::Codex, None)
        .expect("create Codex session");

    let (events, handler) = common::collect_events();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach Codex session");
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
    assert!(recovered.resumable, "the dead Codex row offers resume");
    assert!(
        recovered.peer_session_id.is_some(),
        "the thread handle survived the restart"
    );

    let resumed = client
        .session_resume(
            Persistence {
                // The tag only unwraps the handle; admission is the journal
                // row through `Provider::resumable()`. The app's own bridge
                // sends the same `Acp` tag for every family.
                kind: PersistenceKind::Acp {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect("resume recovered Codex session");
    match resumed {
        ResumeResult::Resumed { session } => {
            assert!(matches!(
                session.state,
                SessionState::Live { generation: 2 }
            ));
        }
        ResumeResult::NotSupported => panic!("Codex resume answered NotSupported"),
        ResumeResult::Failed { message } => panic!("Codex resume failed: {message}"),
    }

    let (events, handler) = common::collect_events();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach resumed Codex session");
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
        .session_create(None, SessionKind::Codex, None)
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
    client
        .session_close(&session.id)
        .expect("close Codex session");
    client
        .session_close(&control.id)
        .expect("close the control session");
}

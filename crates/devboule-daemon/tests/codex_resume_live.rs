//! The proof stage 2 stands on for Codex: the PROVIDER's thread continues.
//!
//! A real `codex app-server`, a primed context word, a killed daemon, a
//! `thread/resume`, a follow-up that only the earlier thread can answer. The
//! stub battery proves the daemon seam; this proves the other side of it.
//!
//! The prime asks for a word **minted for this run**, and the follow-up asks
//! for it **reversed**: the answer is derived, not repeated, so no earlier
//! run, no cached reply and no replayed journal snapshot can contain it. The
//! follow-up is read in the resumed generation only — a replayed row keeps
//! the generation it was written under — and it must spell the exact reversed
//! word and end its turn. A second session, with no context at all, is asked
//! the same question as a negative control — it must fail the same assertion,
//! which is what makes the probe discriminating.
//!
//! Ignored by default: it spends three small model calls and minutes of wall
//! time, and needs an authenticated Codex CLI. Run it explicitly:
//! `cargo test -p devboule-daemon --test codex_resume_live -- --ignored --nocapture`

#![cfg(windows)]

#[path = "claude_common/mod.rs"]
mod common;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::Observed;
use devboule_protocol::NoticeSeverity;
use devboule_protocol::{
    Persistence, PersistenceKind, ResumeResult, SessionEvent, SessionKind, SessionState,
};
use rusqlite::Connection;

/// The generation a session created by this test starts in.
const FIRST_GENERATION: u64 = 1;

/// The follow-up, verbatim: the resumed session and the negative control are
/// asked the same question, so the two answers differ only by context.
const FOLLOW_UP: &str =
    "What exact word did I ask you to reply with in my first message? Reply with that word spelled backwards, and nothing else.";

/// This run's word: never primed before, eight letters like the fixed word it
/// replaces, drawn from an alphabet with no glyph a reader can confuse. From
/// the clock, the process id and a counter — no dependency, and no run can
/// share another run's token.
fn fresh_word() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    static ATTEMPT: AtomicU64 = AtomicU64::new(0);
    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0);
    let mut state = clock.rotate_left(17)
        ^ (u64::from(std::process::id()) << 32)
        ^ ATTEMPT
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (0..8)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ALPHABET[(state >> 33) as usize % ALPHABET.len()] as char
        })
        .collect()
}

fn reversed(word: &str) -> String {
    word.chars().rev().collect()
}

fn agent_texts<'a>(events: impl Iterator<Item = &'a Observed>) -> String {
    events
        .filter_map(|observed| match &observed.event {
            SessionEvent::AgentMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The agent's text in one generation. History keeps the generation it was
/// written under, so the replayed prime turn cannot leak into the follow-up's
/// answer, and a late replay frame cannot satisfy it either.
fn texts_in(events: &[Observed], generation: u64) -> String {
    agent_texts(
        events
            .iter()
            .filter(|observed| observed.generation == generation),
    )
}

/// The generations in the order the collector saw them, one entry per change:
/// printed so a run says which generation a replayed turn arrived under.
fn generations_in_order(events: &[Observed]) -> Vec<u64> {
    let mut seen: Vec<u64> = Vec::new();
    for observed in events {
        if seen.last() != Some(&observed.generation) {
            seen.push(observed.generation);
        }
    }
    seen
}

/// The answer stripped to what it says: letters only, uppercased. Prose
/// around the word fails; punctuation around it does not.
fn answer_only(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_uppercase()
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

fn finished_in(events: &[Observed], generation: u64) -> bool {
    events.iter().any(|observed| {
        observed.generation == generation
            && matches!(observed.event, SessionEvent::AgentFinished { .. })
    })
}

/// A warning notice is how Codex reports a turn that failed: the wait must
/// fail with that sentence instead of outliving the turn by ten minutes.
fn failure_notice(events: &[Observed]) -> Option<String> {
    events.iter().find_map(|observed| match &observed.event {
        SessionEvent::SessionNotice { text, severity } if *severity == NoticeSeverity::Warning => {
            Some(text.clone())
        }
        _ => None,
    })
}

/// Wait until `done` holds over the collected events. A timeout — or a failure
/// notice — panics with everything that did arrive, so a failing run says why.
fn wait_for_events(
    events: &Mutex<Vec<Observed>>,
    what: &str,
    mut done: impl FnMut(&[Observed]) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let notice = {
            // Decide on a snapshot taken under the lock, released before the
            // sleep: a panic while holding the guard would poison the reader
            // thread parked behind it.
            let collected = events.lock().expect("events lock");
            if done(&collected) {
                return;
            }
            failure_notice(&collected)
        };
        if let Some(text) = notice {
            panic!("{what}: the provider reported {text}");
        }
        if Instant::now() > deadline {
            let collected = events.lock().unwrap_or_else(|error| error.into_inner());
            panic!("{what}: {:?}", agent_texts(collected.iter()));
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

    let word = fresh_word();
    let expected = reversed(&word);
    let (events, handler) = common::collect_observed();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach Codex session");
    client
        .session_send(
            &session.id,
            &format!("Reply with exactly the word {word} and nothing else."),
        )
        .expect("prime the conversation");
    wait_for_events(&events, "the prime was never answered", |events| {
        finished_in(events, FIRST_GENERATION) && spells(&texts_in(events, FIRST_GENERATION), &word)
    });
    eprintln!("this run's word: {word:?} (reversed: {expected:?})");

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
    let resumed_generation = match resumed {
        ResumeResult::Resumed { session } => match session.state {
            SessionState::Live { generation } => generation,
            other => panic!("a resumed Codex row is live, not {other:?}"),
        },
        ResumeResult::NotSupported => panic!("Codex resume answered NotSupported"),
        ResumeResult::Failed { message } => panic!("Codex resume failed: {message}"),
    };
    assert_eq!(
        resumed_generation,
        FIRST_GENERATION + 1,
        "a resume opens the next generation"
    );

    let (events, handler) = common::collect_observed();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach resumed Codex session");
    // The attach replays our own journal. Wait for the whole replayed prime
    // turn (its answer plus its end marker), then empty the collector: from
    // here the probe watches only what arrives after the replay. Without the
    // drain, a snapshot the daemon rewinds could satisfy the assertion below —
    // the false proof this test exists to rule out.
    wait_for_events(&events, "the journal replay never landed", |events| {
        finished_in(events, FIRST_GENERATION) && spells(&texts_in(events, FIRST_GENERATION), &word)
    });
    {
        let collected = events.lock().expect("events lock");
        eprintln!(
            "replay generations before the drain: {:?}",
            generations_in_order(&collected)
        );
    }
    events.lock().expect("events lock").clear();

    client
        .session_send(&session.id, FOLLOW_UP)
        .expect("follow-up after resume");
    wait_for_events(
        &events,
        "the context did not survive the resume",
        |events| {
            finished_in(events, resumed_generation)
                && answer_only(&texts_in(events, resumed_generation)) == expected
        },
    );
    let follow_up = {
        let collected = events.lock().expect("events lock");
        eprintln!(
            "follow-up generations: {:?}",
            generations_in_order(&collected)
        );
        answer_only(&texts_in(&collected, resumed_generation))
    };
    assert_eq!(
        follow_up, expected,
        "the follow-up spells this run's word backwards"
    );
    eprintln!("follow-up answered the derived word: {follow_up:?}");

    // The negative control: the same derived question to a session with no
    // context. It must fail the assertion the resumed session just passed,
    // or the assertion proves nothing about context.
    let control = client
        .session_create(None, SessionKind::Codex, None)
        .expect("create the control session");
    let (control_events, control_handler) = common::collect_observed();
    client
        .session_attach(&control.id, None, control_handler)
        .expect("attach the control session");
    client
        .session_send(&control.id, FOLLOW_UP)
        .expect("ask the control session");
    wait_for_events(&control_events, "the control never answered", |events| {
        finished_in(events, FIRST_GENERATION)
    });
    let control_text = texts_in(
        &control_events.lock().expect("events lock"),
        FIRST_GENERATION,
    );
    assert!(
        !spells(&control_text, &expected),
        "an empty-context agent must not produce this run's word backwards; it answered {control_text:?}"
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

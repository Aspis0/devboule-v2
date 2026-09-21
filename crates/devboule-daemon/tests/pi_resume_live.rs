//! The proof stage 3 stands on for Pi: the PROVIDER's session continues.
//!
//! A real `pi` CLI on its rpc wire, a primed context word, a killed daemon, a
//! `--session <id>` resume, a follow-up that only the earlier session can
//! answer. The stub battery proves the daemon seam; this proves the other
//! side of it.
//!
//! The prime asks for a fixed eight-letter word, and the follow-up asks
//! for a fact **derived** from it — how many letters it has — so a replayed
//! journal snapshot cannot contain the answer, and neither can a provider that
//! came back without its conversation. (The sibling tests ask for the word
//! **reversed**; this family's default model was measured answering `DRIEBULB`
//! for `BLUEBIRD`, a transposition of the right reversal, so the exact-string
//! probe is too sharp here. The count is not: it is one digit, it appears
//! nowhere in the transcript, and it still cannot be produced without the
//! word.)
//!
//! A word minted per run was tried here and is not kept: measured on this box,
//! the same model that still held the conversation counted a minted
//! `KZNDZSHC` as **7** letters, so freshness made the probe red for a provider
//! that had resumed correctly. That is a false proof of the wrong kind — the
//! failure mode belongs to the model's arithmetic, not to the resume. The
//! discriminating power here comes from the negative control (a fresh session
//! asked the same question), which the sibling probes do not need because a
//! reversed word cannot be guessed the way a digit can.
//!
//! The follow-up is read in the resumed generation only — a replayed row
//! keeps the generation it was written under — and it must both spell the
//! exact digit and end its turn. A second session, with no context at all, is
//! asked the same question as a negative control — it must fail the same
//! assertion, which is what makes the probe discriminating.
//!
//! The test also measures where the provider keeps the conversation: pi's own
//! session file must still exist after the daemon restart, or `--session <id>`
//! would resolve to nothing. It is looked up under pi's own directory rule,
//! restated here so the two cannot disagree silently.
//!
//! Ignored by default: it spends three small model calls and minutes of wall
//! time, and needs an authenticated pi CLI. Run it explicitly:
//! `cargo test -p devboule-daemon --test pi_resume_live -- --ignored --nocapture`

#![cfg(windows)]

#[path = "claude_common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::Observed;
use devboule_daemon::DaemonClient;
use devboule_protocol::{
    Persistence, PersistenceKind, ResumeResult, SessionEvent, SessionKind, SessionState,
};
use rusqlite::Connection;

/// The generation a session created by this test starts in.
const FIRST_GENERATION: u64 = 1;

/// The word the prime asks for: eight letters, two of them B, so a sloppy
/// count lands on 7 or 9 rather than on the fingerprinted digit.
const PRIME_WORD: &str = "BLUEBIRD";
/// Never spelled by either prompt: only a provider that still holds the
/// conversation knows the word the count belongs to, and no replay of our
/// journal contains this digit.
const LETTER_COUNT: &str = "8";

/// The follow-up, verbatim: the resumed session and the negative control are
/// asked the same question, so the two answers differ only by context.
const FOLLOW_UP: &str =
    "How many letters are in the word I asked you to reply with in my first message? Reply with only the number, using digits.";

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

/// The answer stripped to what it says: letters and digits only, uppercased.
/// Prose around the digit fails; punctuation around it does not.
fn answer_only(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_uppercase()
}

/// The streamed word arrives in pieces (`B`, `LUEBIRD`); it is there once the
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

fn agent_error(events: &[Observed]) -> Option<String> {
    events.iter().find_map(|observed| match &observed.event {
        SessionEvent::AgentError { message } => Some(message.clone()),
        _ => None,
    })
}

/// Wait until `done` holds over the collected events. A timeout — or an agent
/// error — panics with everything that did arrive, so a failing run says why.
fn wait_for_events(
    events: &Mutex<Vec<Observed>>,
    what: &str,
    mut done: impl FnMut(&[Observed]) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let error = {
            // Decide on a snapshot taken under the lock, released before the
            // sleep: a panic while holding the guard would poison the reader
            // thread parked behind it.
            let collected = events.lock().expect("events lock");
            if done(&collected) {
                return;
            }
            agent_error(&collected)
        };
        if let Some(message) = error {
            panic!("{what}: the provider reported {message}");
        }
        if Instant::now() > deadline {
            let collected = events.lock().unwrap_or_else(|error| error.into_inner());
            panic!("{what}: {:?}", agent_texts(collected.iter()));
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

/// Where pi keeps a cwd's sessions: `~/.pi/agent/sessions/--<encoded>--`, the
/// cwd with its separators and drive colon folded to dashes. The rule is
/// restated here so the test fails, not passes hollow, if the provider moves
/// the file the resume depends on.
fn session_dir(home: &Path, cwd: &Path) -> PathBuf {
    let folded = cwd
        .to_string_lossy()
        .trim_start_matches(['/', '\\'])
        .replace(['/', '\\', ':'], "-");
    home.join(".pi")
        .join("agent")
        .join("sessions")
        .join(format!("--{folded}--"))
}

fn session_file(dir: &Path, peer: &str) -> Option<PathBuf> {
    let suffix = format!("_{peer}.jsonl");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
        })
}

#[test]
#[ignore = "three real model calls; needs an authenticated pi CLI"]
fn pi_resume_continues_the_providers_session() {
    let _test_lock = common::lock_tests();
    std::env::remove_var("DEVBOULE_PI_COMMAND");
    // The daemon's rpc handshake budget is 15 s by default, and a cold pi
    // start on this box has been measured past it (the first run of this test
    // died on "Pi permission channel handshake timed out"). The knob exists
    // for exactly this; the harness inherits this process's environment, so
    // both daemon generations see it.
    std::env::set_var("DEVBOULE_PI_HANDSHAKE_TIMEOUT_MS", "60000");

    let mut harness = common::Harness::spawn();
    let client = harness.client();
    let session = client
        .session_create(None, SessionKind::Pi, None)
        .expect("create Pi session");

    let word = PRIME_WORD;
    let count = LETTER_COUNT;
    let (events, handler) = common::collect_observed();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach Pi session");
    client
        .session_send(
            &session.id,
            &format!("Reply with exactly the word {word} and nothing else."),
        )
        .expect("prime the conversation");
    wait_for_events(&events, "the prime was never answered", |events| {
        finished_in(events, FIRST_GENERATION) && spells(&texts_in(events, FIRST_GENERATION), word)
    });
    eprintln!("the prime word: {word:?}, {count} letters");

    // The provider's own file, named by the id the daemon stored off the wire.
    let peer = peer_of(&client, &session.id);
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .expect("home directory");
    // Sessions created without a workspace start in the daemon's own working
    // directory, and the resume stages that same directory back.
    let daemon_cwd = std::env::current_dir().expect("daemon working directory");
    let dir = session_dir(&home, &daemon_cwd);
    let history = session_file(&dir, &peer)
        .unwrap_or_else(|| panic!("no session file for {peer} under {}", dir.display()));
    eprintln!("provider session file: {}", history.display());

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
    assert!(recovered.resumable, "the dead Pi row offers resume");
    assert_eq!(
        recovered.peer_session_id.as_deref(),
        Some(peer.as_str()),
        "the session id survived the restart"
    );
    // The trap the Codex slice paid for: a session store the daemon owns and
    // sweeps would leave this resume with nothing to load. The file is the
    // human's, under the human's home, and it is still there.
    assert!(
        history.is_file(),
        "the provider's conversation survived the daemon restart: {}",
        history.display()
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
        .expect("resume recovered Pi session");
    let resumed_generation = match resumed {
        ResumeResult::Resumed { session } => match session.state {
            SessionState::Live { generation } => generation,
            other => panic!("a resumed Pi row is live, not {other:?}"),
        },
        ResumeResult::NotSupported => panic!("Pi resume answered NotSupported"),
        ResumeResult::Failed { message } => panic!("Pi resume failed: {message}"),
    };
    assert_eq!(
        resumed_generation,
        FIRST_GENERATION + 1,
        "a resume opens the next generation"
    );

    let (events, handler) = common::collect_observed();
    client
        .session_attach(&session.id, None, handler)
        .expect("attach resumed Pi session");
    // The attach replays our own journal. Wait for the whole replayed prime
    // turn (its answer plus its end marker), then empty the collector: from
    // here the probe watches only what arrives after the replay. Without the
    // drain, a snapshot the daemon rewinds could satisfy the assertion below —
    // the false proof this test exists to rule out.
    wait_for_events(&events, "the journal replay never landed", |events| {
        finished_in(events, FIRST_GENERATION) && spells(&texts_in(events, FIRST_GENERATION), word)
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
                && answer_only(&texts_in(events, resumed_generation)) == count
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
        follow_up, count,
        "the follow-up derives this run's letter count"
    );
    eprintln!("follow-up answered the derived count: {follow_up:?}");

    // The negative control: the same derived question to a session with no
    // context. It must fail the assertion the resumed session just passed,
    // or the assertion proves nothing about context.
    let control = client
        .session_create(None, SessionKind::Pi, None)
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
        !spells(&control_text, count),
        "an empty-context agent must not produce the derived count; it answered {control_text:?}"
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
    client.session_close(&session.id).expect("close Pi session");
    client
        .session_close(&control.id)
        .expect("close the control session");
    // Best-effort: our probe sessions do not belong in the human's picker.
    let _ = std::fs::remove_file(&history);
    if let Some(control_history) = session_file(&dir, &control_peer) {
        let _ = std::fs::remove_file(control_history);
    }
}

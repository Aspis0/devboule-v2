//! The transcript search behind the sidebar's Ctrl+K: the words the person and
//! the agent wrote, found in each session's own journal.
//!
//! It is a bounded scan, not an index, and it never runs on the request that
//! asked for it. A scanner thread reads the owner's sessions newest first and
//! fills a shared collector; the caller waits at most [`SEARCH_TIME_BUDGET`]
//! and answers with what the collector holds. Past the budget the scanner is
//! told to stop, and `truncated` says so: a partial scan is never presented as
//! a complete one. A new search cancels the one before it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::{SearchAuthor, SessionEvent, SessionSearchHit, UserMessageAuthor};

use crate::journal::Journal;

/// Most sessions one search reads, newest first.
pub(crate) const SEARCH_SESSION_CAP: usize = 200;

/// Most hits one search returns, newest first.
pub(crate) const SEARCH_HIT_CAP: usize = 50;

/// Most matches one session contributes, newest first.
pub(crate) const SEARCH_SESSION_CANDIDATE_CAP: usize = 50;

/// The largest transcript (stored payload bytes) a search replays into memory.
/// A bigger one is skipped and the answer says so.
pub(crate) const SEARCH_SESSION_BYTES_CAP: u64 = 64 * 1024 * 1024;

/// Most matches one scan collects before it stops and sorts; reaching it is a
/// cap like any other and sets `truncated`.
pub(crate) const SEARCH_CANDIDATE_CAP: usize = 500;

/// The wall-clock budget of one search, measured from when it starts. The
/// answer waits for this long at most, whatever the scanner is still doing.
pub(crate) const SEARCH_TIME_BUDGET: Duration = Duration::from_millis(1_500);

/// Characters of context kept on each side of a match in its snippet.
const SNIPPET_CONTEXT: usize = 60;

/// One search's answer: the hits, newest first, and whether a cap, the time
/// budget, a cancellation or an unreadable journal cut the scan short.
pub(crate) struct TranscriptSearch {
    pub(crate) hits: Vec<SessionSearchHit>,
    /// More matches exist than the answer holds: a cap was reached.
    pub(crate) truncated: bool,
    /// The scan did not read everything it should have: the time budget ran
    /// out, it was superseded, a transcript could not be read, or one was
    /// skipped as too large. The answer may be missing matches for that reason.
    pub(crate) stopped_early: bool,
}

/// A match with the time its event was written, for the newest-first order.
struct Candidate {
    at_ms: u64,
    hit: SessionSearchHit,
}

#[derive(Default)]
struct Collector {
    candidates: Vec<Candidate>,
    truncated: bool,
    stopped_early: bool,
    finished: bool,
}

/// One search in flight. The scanner owns the journal reads; the caller only
/// waits and reads the collector.
pub(crate) struct SearchRun {
    cancel: Arc<AtomicBool>,
    collector: Arc<Mutex<Collector>>,
    done: mpsc::Receiver<()>,
    deadline: Instant,
}

impl SearchRun {
    /// Starts scanning `query` in `owner`'s transcripts. A blank query has
    /// nothing to scan, so there is no run to wait on.
    pub(crate) fn start(
        journal: Arc<Journal>,
        owner: &str,
        query: &str,
        cancel: Arc<AtomicBool>,
    ) -> Option<SearchRun> {
        let needle = query.trim().to_string();
        if needle.is_empty() {
            return None;
        }
        let deadline = Instant::now() + SEARCH_TIME_BUDGET;
        let collector = Arc::new(Mutex::new(Collector::default()));
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let scanner_collector = Arc::clone(&collector);
        let scanner_cancel = Arc::clone(&cancel);
        let owner = owner.to_string();
        let spawned = std::thread::Builder::new()
            .name("daemon-transcript-scan".to_string())
            .spawn(move || {
                // The sender drops when the scan ends, however it ends, so the
                // waiter never waits on a scanner that is gone.
                let _done = done_tx;
                scan(
                    &journal,
                    &owner,
                    &needle,
                    &scanner_cancel,
                    &scanner_collector,
                );
            });
        if spawned.is_err() {
            // No scanner means no hits, and the answer must say the search did
            // not run to completion.
            if let Ok(mut state) = collector.lock() {
                state.stopped_early = true;
                state.finished = true;
            }
        }
        Some(SearchRun {
            cancel,
            collector,
            done: done_rx,
            deadline,
        })
    }

    /// Waits for the scan up to the budget, then answers with what it found.
    pub(crate) fn wait(self) -> TranscriptSearch {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        match self.done.recv_timeout(remaining) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {}
            Err(RecvTimeoutError::Timeout) => {
                // Stop the scanner at its next check. Whatever it found so far
                // is the answer, and it is partial.
                self.cancel.store(true, Ordering::Release);
            }
        }
        let Ok(mut state) = self.collector.lock() else {
            return TranscriptSearch {
                hits: Vec::new(),
                truncated: false,
                stopped_early: true,
            };
        };
        let mut truncated = state.truncated;
        // A scan still running at the budget was stopped, whatever it found.
        let stopped_early = state.stopped_early || !state.finished;
        let mut candidates = std::mem::take(&mut state.candidates);
        candidates.sort_by(|left, right| {
            right
                .at_ms
                .cmp(&left.at_ms)
                .then_with(|| right.hit.session_id.cmp(&left.hit.session_id))
                .then_with(|| right.hit.generation.cmp(&left.hit.generation))
                .then_with(|| right.hit.seq.cmp(&left.hit.seq))
        });
        if candidates.len() > SEARCH_HIT_CAP {
            truncated = true;
        }
        let hits = candidates
            .into_iter()
            .take(SEARCH_HIT_CAP)
            .map(|candidate| candidate.hit)
            .collect();
        TranscriptSearch {
            hits,
            truncated,
            stopped_early,
        }
    }
}

/// The scanner: sessions newest first, each session's events newest first,
/// checking the cancel flag before every session and every event. Each
/// session's matches reach the collector as one batch, so a waiter that gives
/// up mid-scan still reads everything found before that session.
fn scan(
    journal: &Journal,
    owner: &str,
    needle: &str,
    cancel: &AtomicBool,
    collector: &Mutex<Collector>,
) {
    // The scan reads on its own connection: the writer thread is never asked
    // for rows, so a search cannot delay an append.
    let finish = |more: bool, early: bool| {
        if let Ok(mut state) = collector.lock() {
            state.truncated |= more;
            state.stopped_early |= early;
            state.finished = true;
        }
    };
    let Ok(reader) = journal.open_reader() else {
        finish(false, true);
        return;
    };
    let Ok(records) = crate::journal::list_sessions_on(&reader) else {
        finish(false, true);
        return;
    };
    let mut sessions: Vec<_> = records
        .into_iter()
        .filter(|record| record.owner == owner)
        .collect();
    sessions.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
    let mut truncated = sessions.len() > SEARCH_SESSION_CAP;
    let mut stopped_early = false;
    for record in sessions.iter().take(SEARCH_SESSION_CAP) {
        if cancel.load(Ordering::Acquire) {
            stopped_early = true;
            break;
        }
        // A transcript past the size cap is not replayed into memory: the answer
        // says the search did not read it, rather than reading it whole.
        if record.payload_bytes > SEARCH_SESSION_BYTES_CAP {
            stopped_early = true;
            continue;
        }
        let Ok(replay) = crate::journal::replay_session_on(&reader, &record.id) else {
            // An unreadable session is a gap in the answer, never a silent
            // "no match".
            stopped_early = true;
            continue;
        };
        let mut found: Vec<Candidate> = Vec::new();
        for (index, (event, (generation, seq))) in replay
            .events
            .iter()
            .zip(replay.event_seqs.iter())
            .enumerate()
            .rev()
        {
            if cancel.load(Ordering::Acquire) {
                stopped_early = true;
                break;
            }
            // The newest matches of a session are the ones kept: once the
            // per-session cap is full, older matches are not built at all.
            if found.len() >= SEARCH_SESSION_CANDIDATE_CAP {
                truncated = true;
                break;
            }
            let Some((author, message_id, text)) = searchable(event) else {
                continue;
            };
            let Some(snippet) = snippet_around(text, needle) else {
                continue;
            };
            let at_ms = replay
                .event_ts_ms
                .get(index)
                .copied()
                .flatten()
                .unwrap_or(0);
            found.push(Candidate {
                at_ms,
                hit: SessionSearchHit {
                    session_id: record.id.clone(),
                    generation: *generation,
                    seq: *seq,
                    message_id,
                    author,
                    snippet,
                },
            });
        }
        if found.is_empty() {
            continue;
        }
        let Ok(mut state) = collector.lock() else {
            finish(false, true);
            return;
        };
        if state.candidates.len() + found.len() > SEARCH_CANDIDATE_CAP {
            let room = SEARCH_CANDIDATE_CAP.saturating_sub(state.candidates.len());
            state.candidates.extend(found.into_iter().take(room));
            truncated = true;
            break;
        }
        state.candidates.extend(found);
    }
    finish(truncated, stopped_early);
}

/// The words of an event a search reads: what the person typed, and what the
/// agent said to them. Tool chatter, notices and a subagent's own messages are
/// not transcript the person wrote or read as the conversation.
fn searchable(event: &SessionEvent) -> Option<(SearchAuthor, Option<String>, &str)> {
    match event {
        SessionEvent::AgentUserMessage {
            message_id,
            text,
            author: UserMessageAuthor::Human,
            ..
        } => Some((SearchAuthor::Person, message_id.clone(), text.as_str())),
        SessionEvent::AgentMessage {
            message_id,
            text,
            parent_tool_use_id: None,
            ..
        } => Some((SearchAuthor::Agent, message_id.clone(), text.as_str())),
        _ => None,
    }
}

/// The first case-insensitive match of `needle` in `text`, as a snippet with
/// its context on both sides, or `None` when the text does not hold it.
fn snippet_around(text: &str, needle: &str) -> Option<String> {
    let (start, end) = find_case_insensitive(text, needle)?;
    let before: Vec<char> = text[..start].chars().collect();
    let after: Vec<char> = text[end..].chars().collect();
    let lead = before.len().saturating_sub(SNIPPET_CONTEXT);
    let tail = after.len().min(SNIPPET_CONTEXT);
    let mut snippet = String::new();
    if lead > 0 {
        snippet.push('…');
    }
    snippet.extend(before[lead..].iter());
    snippet.push_str(&text[start..end]);
    snippet.extend(after[..tail].iter());
    if tail < after.len() {
        snippet.push('…');
    }
    Some(collapse_whitespace(&snippet))
}

/// The byte range of the first case-insensitive occurrence of `needle` in
/// `haystack`. Each character is compared through its lowercase form, so a
/// character whose lowercase is several characters still matches its own
/// original bytes.
fn find_case_insensitive(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    let wanted: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if wanted.is_empty() {
        return None;
    }
    // (original byte start, original byte length, one lowercase character)
    let mut folded: Vec<(usize, usize, char)> = Vec::new();
    for (at, original) in haystack.char_indices() {
        for lower in original.to_lowercase() {
            folded.push((at, original.len_utf8(), lower));
        }
    }
    let window = wanted.len();
    if folded.len() < window {
        return None;
    }
    for first in 0..=folded.len() - window {
        let matches = wanted
            .iter()
            .enumerate()
            .all(|(offset, lower)| folded[first + offset].2 == *lower);
        if matches {
            let (start, _, _) = folded[first];
            let (last_start, last_len, _) = folded[first + window - 1];
            return Some((start, last_start + last_len));
        }
    }
    None
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{agent_report_record, agent_report_record_at, new_session_record};
    use devboule_protocol::SessionKind;

    #[test]
    fn a_match_is_found_whatever_its_case() {
        assert_eq!(
            find_case_insensitive("Deploy the Kalsa site", "kalsa"),
            Some((11, 16))
        );
        assert_eq!(find_case_insensitive("DEPLOY", "deploy"), Some((0, 6)));
        assert_eq!(find_case_insensitive("nothing here", "kalsa"), None);
        assert_eq!(find_case_insensitive("anything", ""), None);
    }

    #[test]
    fn a_match_keeps_its_own_bytes_when_the_lowercase_form_is_longer() {
        // "İ" lowercases to two characters; the match must still end on the
        // original character's boundary, so the snippet slices cleanly.
        let text = "İstanbul plan";
        let (start, end) = find_case_insensitive(text, "plan").expect("the word is there");
        assert_eq!(&text[start..end], "plan");
    }

    #[test]
    fn the_snippet_shows_the_match_with_context_and_marks_cut_ends() {
        let text = format!("{} needle {}", "a".repeat(200), "b".repeat(200));
        let snippet = snippet_around(&text, "NEEDLE").expect("the match is found");
        assert!(snippet.starts_with('…'));
        assert!(snippet.ends_with('…'));
        assert!(snippet.contains("needle"));
        assert!(snippet.chars().count() <= 2 * SNIPPET_CONTEXT + "needle".len() + 2);
    }

    #[test]
    fn a_short_text_is_shown_whole_and_its_newlines_collapse() {
        assert_eq!(
            snippet_around("first line\nsecond  line", "second").as_deref(),
            Some("first line second line")
        );
    }

    #[test]
    fn only_the_person_and_the_agent_words_are_searched() {
        let person: SessionEvent = serde_json::from_value(serde_json::json!({
            "type": "agent_user_message",
            "text": "ship it",
        }))
        .expect("a human message event");
        assert_eq!(
            searchable(&person).map(|(author, _, _)| author),
            Some(SearchAuthor::Person)
        );
        let daemon_note: SessionEvent = serde_json::from_value(serde_json::json!({
            "type": "agent_user_message",
            "text": "the daemon's own line",
            "author": "agent",
        }))
        .expect("a daemon-authored message event");
        assert!(searchable(&daemon_note).is_none());
        let subagent: SessionEvent = serde_json::from_value(serde_json::json!({
            "type": "agent_message",
            "text": "a child's chatter",
            "parentToolUseId": "tool-1",
        }))
        .expect("a subagent message event");
        assert!(searchable(&subagent).is_none());
    }

    /// A journal with two owners' sessions, one of them holding the word.
    fn searchable_journal() -> (std::path::PathBuf, Arc<Journal>) {
        let dir = crate::test_dirs::test_temp_dir("devboule-transcript-search");
        let journal = Journal::open(&dir.join("journal.sqlite")).expect("journal");
        for (id, owner) in [("s.mine", "owner"), ("s.theirs", "someone")] {
            journal
                .upsert_blocking(new_session_record(
                    id,
                    owner,
                    None,
                    SessionKind::Claude,
                    "a session",
                ))
                .expect("session row");
            journal.start_generation(id, 1).expect("generation");
        }
        let person: SessionEvent = serde_json::from_value(serde_json::json!({
            "type": "agent_user_message",
            "text": "Deploy the Kalsa site",
            "messageId": "m1",
        }))
        .expect("a human message");
        let agent: SessionEvent = serde_json::from_value(serde_json::json!({
            "type": "agent_message",
            "text": "The Kalsa build passed",
            "messageId": "m2",
        }))
        .expect("an agent message");
        let stranger: SessionEvent = serde_json::from_value(serde_json::json!({
            "type": "agent_user_message",
            "text": "Kalsa, but someone else's",
        }))
        .expect("another owner's message");
        for (session, seq, event) in [
            ("s.mine", 1, &person),
            ("s.mine", 2, &agent),
            ("s.theirs", 1, &stranger),
        ] {
            let record = agent_report_record(session, 1, seq, event).expect("a record");
            journal
                .append_blocking(record)
                .expect("the event is journalled");
        }
        (dir, Arc::new(journal))
    }

    fn search(journal: &Arc<Journal>, owner: &str, query: &str) -> TranscriptSearch {
        SearchRun::start(
            Arc::clone(journal),
            owner,
            query,
            Arc::new(AtomicBool::new(false)),
        )
        .map(SearchRun::wait)
        .unwrap_or(TranscriptSearch {
            hits: Vec::new(),
            truncated: false,
            stopped_early: false,
        })
    }

    #[test]
    fn search_reads_the_owners_transcripts_newest_first_with_its_journal_position() {
        let (dir, journal) = searchable_journal();
        let found = search(&journal, "owner", "kalsa");
        assert!(!found.truncated, "nothing stopped the scan");
        let rows: Vec<(&str, u64, u64, SearchAuthor, Option<&str>)> = found
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.session_id.as_str(),
                    hit.generation,
                    hit.seq,
                    hit.author,
                    hit.message_id.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("s.mine", 1, 2, SearchAuthor::Agent, Some("m2")),
                ("s.mine", 1, 1, SearchAuthor::Person, Some("m1")),
            ],
            "the other owner's session is never read"
        );
        assert!(found.hits[0].snippet.contains("Kalsa build"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hits_are_ordered_by_when_they_were_written_not_by_their_session() {
        use crate::journal::agent_report_record_at;
        let dir = crate::test_dirs::test_temp_dir("devboule-transcript-order");
        let journal = Journal::open(&dir.join("journal.sqlite")).expect("journal");
        // s.old was touched last, but its match was written first.
        for (id, updated_at_ms, written_at, text) in [
            ("s.old", 2_000_u64, 100_u64, "kalsa from long ago"),
            ("s.new", 1_000_u64, 500_u64, "kalsa from just now"),
        ] {
            let mut row = new_session_record(id, "owner", None, SessionKind::Claude, "a session");
            row.updated_at_ms = updated_at_ms;
            journal.upsert_blocking(row).expect("session row");
            journal.start_generation(id, 1).expect("generation");
            let event: SessionEvent = serde_json::from_value(serde_json::json!({
                "type": "agent_user_message",
                "text": text,
            }))
            .expect("a human message");
            let record = agent_report_record_at(id, 1, 1, &event, written_at).expect("a record");
            journal
                .append_blocking(record)
                .expect("the event is journalled");
        }
        let found = search(&Arc::new(journal), "owner", "kalsa");
        let order: Vec<&str> = found
            .hits
            .iter()
            .map(|hit| hit.session_id.as_str())
            .collect();
        assert_eq!(
            order,
            vec!["s.new", "s.old"],
            "the newest match comes first"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_capped_answer_is_truncated_not_stopped_early() {
        let dir = crate::test_dirs::test_temp_dir("devboule-transcript-cap");
        let journal = Journal::open(&dir.join("journal.sqlite")).expect("journal");
        journal
            .upsert_blocking(new_session_record(
                "s.many",
                "owner",
                None,
                SessionKind::Claude,
                "a session",
            ))
            .expect("session row");
        journal.start_generation("s.many", 1).expect("generation");
        for seq in 1..=(SEARCH_SESSION_CANDIDATE_CAP as u64 + 5) {
            let event: SessionEvent = serde_json::from_value(serde_json::json!({
                "type": "agent_user_message",
                "text": format!("kalsa line {seq}"),
            }))
            .expect("a human message");
            let record = agent_report_record_at("s.many", 1, seq, &event, seq).expect("a record");
            journal.append_blocking(record).expect("journalled");
        }
        let found = search(&Arc::new(journal), "owner", "kalsa");
        assert_eq!(
            found.hits.len(),
            SEARCH_HIT_CAP.min(SEARCH_SESSION_CANDIDATE_CAP)
        );
        assert!(found.truncated, "more matches exist than the answer holds");
        assert!(!found.stopped_early, "nothing was skipped or cut short");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_cancelled_scan_stops_and_says_so() {
        let (dir, journal) = searchable_journal();
        let cancel = Arc::new(AtomicBool::new(true));
        let run = SearchRun::start(Arc::clone(&journal), "owner", "kalsa", cancel)
            .expect("a run for a real query");
        let found = run.wait();
        assert!(found.hits.is_empty(), "nothing is read after the cancel");
        assert!(
            found.stopped_early,
            "a cancelled scan is never presented as complete"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_blank_query_starts_no_scan_and_truncates_nothing() {
        let (dir, journal) = searchable_journal();
        assert!(SearchRun::start(
            Arc::clone(&journal),
            "owner",
            "   ",
            Arc::new(AtomicBool::new(false)),
        )
        .is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_budget_bounds_the_answer_even_when_the_scan_never_finishes() {
        // A run whose scanner never reports done: the caller must answer at the
        // budget, flag the answer as partial, and tell the scanner to stop.
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let cancel = Arc::new(AtomicBool::new(false));
        let run = SearchRun {
            cancel: Arc::clone(&cancel),
            collector: Arc::new(Mutex::new(Collector::default())),
            done: done_rx,
            deadline: Instant::now() + Duration::from_millis(50),
        };
        let started = Instant::now();
        let found = run.wait();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the wait is bounded"
        );
        assert!(found.stopped_early);
        assert!(
            cancel.load(Ordering::Acquire),
            "the scanner is told to stop"
        );
        drop(done_tx);
    }
}

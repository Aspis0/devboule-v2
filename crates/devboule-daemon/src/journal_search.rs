//! The transcript search behind the sidebar's Ctrl+K: the words the person and
//! the agent wrote, found in each session's own journal.
//!
//! It is a bounded scan, not an index. Sessions are read newest first, and a
//! session cap, a hit cap and a time budget each stop it; `truncated` says
//! when one of them did, so the app never presents a partial scan as complete.

use std::time::Instant;

use devboule_protocol::{SearchAuthor, SessionEvent, SessionSearchHit, UserMessageAuthor};

use crate::journal::{Journal, JournalError};

/// Most sessions one search reads, newest first.
pub(crate) const SEARCH_SESSION_CAP: usize = 200;

/// Most hits one search returns.
pub(crate) const SEARCH_HIT_CAP: usize = 50;

/// The wall-clock budget of one search. The scan runs on the connection that
/// asked, so it stops here rather than hold that connection for longer.
pub(crate) const SEARCH_TIME_BUDGET: std::time::Duration = std::time::Duration::from_millis(1_500);

/// Characters of context kept on each side of a match in its snippet.
const SNIPPET_CONTEXT: usize = 60;

/// One search's answer: the hits, newest session first, and whether a cap or
/// the time budget stopped the scan before it read everything.
pub(crate) struct TranscriptSearch {
    pub(crate) hits: Vec<SessionSearchHit>,
    pub(crate) truncated: bool,
}

impl Journal {
    /// Searches the transcripts `owner` owns. A blank query reads nothing and
    /// truncates nothing: there is no search to have stopped.
    pub(crate) fn search_transcripts(
        &self,
        owner: &str,
        query: &str,
        deadline: Instant,
    ) -> Result<TranscriptSearch, JournalError> {
        let needle = query.trim();
        if needle.is_empty() {
            return Ok(TranscriptSearch {
                hits: Vec::new(),
                truncated: false,
            });
        }
        let mut sessions: Vec<_> = self
            .list()?
            .into_iter()
            .filter(|record| record.owner == owner)
            .collect();
        sessions.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
        let mut truncated = sessions.len() > SEARCH_SESSION_CAP;
        let mut hits = Vec::new();
        for record in sessions.iter().take(SEARCH_SESSION_CAP) {
            if Instant::now() >= deadline {
                truncated = true;
                break;
            }
            let Ok(replay) = self.replay(&record.id) else {
                continue;
            };
            // Newest entries first: a session's latest words are the likeliest
            // thing the person is looking for.
            for (event, (generation, seq)) in
                replay.events.iter().zip(replay.event_seqs.iter()).rev()
            {
                let Some((author, message_id, text)) = searchable(event) else {
                    continue;
                };
                let Some(snippet) = snippet_around(text, needle) else {
                    continue;
                };
                hits.push(SessionSearchHit {
                    session_id: record.id.clone(),
                    generation: *generation,
                    seq: *seq,
                    message_id,
                    author,
                    snippet,
                });
                if hits.len() >= SEARCH_HIT_CAP {
                    return Ok(TranscriptSearch {
                        hits,
                        truncated: true,
                    });
                }
            }
        }
        Ok(TranscriptSearch { hits, truncated })
    }
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
    fn searchable_journal() -> (std::path::PathBuf, Journal) {
        use crate::journal::{agent_report_record, new_session_record};
        use devboule_protocol::SessionKind;

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
            assert!(journal.try_append(record), "the event is journalled");
        }
        (dir, journal)
    }

    #[test]
    fn search_reads_the_owners_transcripts_newest_entry_first_with_its_journal_position() {
        let (dir, journal) = searchable_journal();
        let deadline = Instant::now() + SEARCH_TIME_BUDGET;
        let search = journal
            .search_transcripts("owner", "kalsa", deadline)
            .expect("the search runs");
        assert!(!search.truncated, "nothing stopped the scan");
        let found: Vec<(&str, u64, u64, SearchAuthor, Option<&str>)> = search
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
            found,
            vec![
                ("s.mine", 1, 2, SearchAuthor::Agent, Some("m2")),
                ("s.mine", 1, 1, SearchAuthor::Person, Some("m1")),
            ],
            "the other owner's session is never read"
        );
        assert!(search.hits[0].snippet.contains("Kalsa build"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_spent_time_budget_stops_the_scan_and_says_so() {
        let (dir, journal) = searchable_journal();
        let search = journal
            .search_transcripts("owner", "kalsa", Instant::now())
            .expect("the search runs");
        assert!(search.hits.is_empty());
        assert!(
            search.truncated,
            "a scan cut short is never presented as complete"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_blank_query_reads_nothing_and_truncates_nothing() {
        let (dir, journal) = searchable_journal();
        let search = journal
            .search_transcripts("owner", "   ", Instant::now() + SEARCH_TIME_BUDGET)
            .expect("the search runs");
        assert!(search.hits.is_empty());
        assert!(!search.truncated);
        std::fs::remove_dir_all(&dir).ok();
    }
}

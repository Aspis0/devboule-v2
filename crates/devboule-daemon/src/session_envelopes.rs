//! The `<devboule-system>` envelope builders: the frame a finish report, an
//! input-required or quiet notice, a permission card and an agent message
//! arrive in, the excerpt and escaping helpers they share, and the constants
//! that bound them.
//!
//! Split out of `session.rs` without a rewrite: every line below this header is
//! byte-identical to its text there, apart from the twelve `pub(super)` markers
//! a caller in the parent module or its sibling tests reaches in for, and the
//! one signature rustfmt re-wrapped because the marker pushed it past 100
//! columns (`agent_message_envelope`).

use super::*;

/// The finish report's envelope (`S5` decision 7).
///
/// The same `<devboule-system>` frame an agent message arrives in — one builder,
/// one escaping rule — with a `kind` line and the structured body. Every value
/// that came from the child goes through [`neutralise_envelope_text`], so the
/// child's own words cannot close the envelope or open a second one, and CR/LF
/// is normalised on the way in: a summary with a lone carriage return cannot
/// forge a line the envelope did not write.
pub(super) fn agent_finished_envelope(
    child_session_id: &str,
    display_name: &str,
    state: AgentTaskState,
    summary: &str,
    artifacts: &[FinishArtifact],
    note: Option<&str>,
    child_origin: &SessionOrigin,
) -> String {
    let clean = |value: &str| neutralise_envelope_text(value);
    let mut body = format!(
        "childSessionId: {}\ndisplayName: {}\nstate: {}\nsummary: {}",
        clean(&single_line_header(child_session_id)),
        clean(&single_line_header(display_name)),
        state.as_str(),
        clean(summary)
    );
    if let Some(note) = note {
        body.push_str(&format!("\nnote: {}", clean(note)));
    }
    let artifacts = serde_json::to_string(artifacts).unwrap_or_else(|_| "[]".to_string());
    body.push_str(&format!("\nartifacts: {}", clean(&artifacts)));
    format!(
        "<devboule-system>\norigin: {}\nrole: daemon\nfrom_agent: {}\nkind: agent_finished\ntimestamp: {}\n{}\n</devboule-system>",
        origin_line(child_origin),
        clean(&single_line_header(child_session_id)),
        unix_millis(),
        body
    )
}

/// The `input_required` notice (`S5` §3): one line saying the child is parked on
/// a card, and deliberately not what the card says.
pub(super) fn agent_input_required_envelope(
    child_session_id: &str,
    display_name: &str,
    child_origin: &SessionOrigin,
) -> String {
    format!(
        "<devboule-system>\norigin: {}\nrole: daemon\nfrom_agent: {}\nkind: agent_input_required\ntimestamp: {}\nchildSessionId: {}\ndisplayName: {}\nstate: input_required\nsummary: This agent is waiting for a person to answer a permission card.\n</devboule-system>",
        origin_line(child_origin),
        neutralise_envelope_text(&single_line_header(child_session_id)),
        unix_millis(),
        neutralise_envelope_text(&single_line_header(child_session_id)),
        neutralise_envelope_text(&single_line_header(display_name))
    )
}

/// The quiet notice: a Working child with no publish for `idle_ms`. A notice,
/// never an action — it names the child and its idle age, carries no child
/// text, and changes nothing about the child. What it cannot distinguish: a
/// model thinking hard, a long build and a wedged process look identical
/// from outside, so the creator decides and the daemon only reports.
pub(super) fn agent_quiet_envelope(
    child_session_id: &str,
    display_name: &str,
    idle_ms: u64,
    child_origin: &SessionOrigin,
) -> String {
    let minutes = idle_ms / 60_000;
    format!(
        "<devboule-system>\norigin: {}\nrole: daemon\nfrom_agent: {}\nkind: agent_quiet\ntimestamp: {}\nchildSessionId: {}\ndisplayName: {}\nstate: working\nidleMs: {}\nsummary: This agent is still working but has produced no output for {minutes} minute(s). It may be thinking, building, or stuck; nothing was stopped.\n</devboule-system>",
        origin_line(child_origin),
        neutralise_envelope_text(&single_line_header(child_session_id)),
        unix_millis(),
        neutralise_envelope_text(&single_line_header(child_session_id)),
        neutralise_envelope_text(&single_line_header(display_name)),
        idle_ms,
    )
}

/// One header line per child-chosen value: a line break in it would impersonate
/// frame structure, so every mandatory break (CR, LF, VT, FF, NEL, U+2028, U+2029)
/// becomes a space before anything else runs. The cap
/// bounds the frame, not the card. `pub(crate)` because the peer-roster
/// boundary (`mcp_peer_agents`) composes the same two passes the card path
/// composes rather than growing a second neutraliser.
pub(crate) fn single_line_header(text: &str) -> String {
    let mut single = String::with_capacity(text.len());
    let mut rest = text.chars().peekable();
    while let Some(next) = rest.next() {
        if crate::text_safety::is_mandatory_line_break(next) {
            if next == '\r' && rest.peek() == Some(&'\n') {
                rest.next();
            }
            single.push(' ');
        } else {
            single.push(next);
        }
    }
    single.chars().take(TITLE_LINE_MAX_CHARS).collect()
}

/// The delegated-surfacing envelope (§4.3, §6.7 of the app contract): the
/// daemon's facts in the header — `cardId`, `toolTitle`, `displayName`, one
/// line each, exactly those keys — and the child's own words fenced between
/// the exact lines `child-said:` and `end child-said`. The fence markers are
/// neutralised inside the excerpt the same way the envelope tags are, so a
/// child that writes a closer into its own words cannot close its quoted
/// block early: the human must see at least as much of the card as the model
/// does.
///
/// **This quoting is a mitigation, not a fix.** The excerpt is text the child
/// chose, entering the creator's prompt; a confused or hostile child can
/// still try to steer its creator in those words. The fence and the system
/// styling exist so the creator's model — and the human reading over its
/// shoulder — can tell whose words they are, and nothing more.
///
/// Every header value is single-line by construction of this builder's
/// inputs (the card id is daemon-minted; the title and name are sanitised
/// below), because a header value carrying a newline would grow the frame a
/// second quoted block — the malformed frame the app refuses rather than
/// half-parse.
pub(super) fn agent_permission_request_envelope(
    child_session_id: &str,
    child_origin: &SessionOrigin,
    card_id: &str,
    tool_title: &str,
    display_name: &str,
    excerpt: &str,
) -> String {
    format!(
        "<devboule-system>\norigin: {}\nrole: daemon\nfrom_agent: {}\nkind: agent_permission_request\ntimestamp: {}\ncardId: {}\ntoolTitle: {}\ndisplayName: {}\nchild-said:\n{}\nend child-said\n</devboule-system>",
        origin_line(child_origin),
        neutralise_envelope_text(&single_line_header(child_session_id)),
        unix_millis(),
        neutralise_envelope_text(&single_line_header(card_id)),
        neutralise_envelope_text(&single_line_header(tool_title)),
        neutralise_envelope_text(&single_line_header(display_name)),
        neutralise_envelope_text(&cap_excerpt_scalars(excerpt)),
    )
}

/// The most characters one child-chosen header line may carry, after
/// newlines became spaces. A card title is provider text of unbounded shape;
/// this bounds the frame, not the card.
pub(crate) const TITLE_LINE_MAX_CHARS: usize = 256;

/// The excerpt cap (§4.3): 512 Unicode **scalar values**, counted on the raw
/// text after CR/LF normalisation and before any escaping, cut at a scalar
/// boundary — never inside one. The escaped wire form may exceed 512 units;
/// the app never re-truncates, so this is the only cut the excerpt gets.
pub(super) fn cap_excerpt_scalars(text: &str) -> String {
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    normalised.chars().take(EXCERPT_MAX_SCALARS).collect()
}

const EXCERPT_MAX_SCALARS: usize = 512;

/// The envelope's `origin:` line for a session's own stored origin. Never read
/// from a connection: the finish hook runs on whatever thread the child's
/// provider ended on.
fn origin_line(origin: &SessionOrigin) -> String {
    match origin.kind {
        SessionOriginKind::Peer => {
            format!("peer:{}", origin.device_id.as_deref().unwrap_or_default())
        }
        SessionOriginKind::Local => "local".to_string(),
        SessionOriginKind::Unknown => "unknown".to_string(),
    }
}

/// The first `FINISH_SUMMARY_CHARS` characters of the child's last message.
///
/// Paseo's number, and characters rather than bytes so the cut cannot land
/// inside one. The whole message is still what gets deposited: the summary is
/// what a person reads in the transcript.
pub(super) fn summary_of(message: Option<&str>) -> String {
    const FINISH_SUMMARY_CHARS: usize = 4000;
    let message = message.unwrap_or_default();
    message.chars().take(FINISH_SUMMARY_CHARS).collect()
}

/// How a child ended, in the vocabulary the finish report uses (`S5` §3).
///
/// A stop reason is the provider's own word for why the turn ended. Only
/// `end_turn` is a completed run: `max_tokens`, `max_turn_requests` and
/// `refusal` all mean the agent stopped short of doing what it was asked, and
/// saying `completed` there would be a claim the provider contradicts. A
/// session that never reported one is judged by its exit: a clean end is
/// `completed`, an unclean one `failed`, and a session the human closed (or one
/// whose daemon died) is `canceled` — it did not report and nothing says it
/// failed.
pub(super) fn child_finish_state(
    session: &Session,
    runtime: &SessionRuntime,
) -> (AgentTaskState, Option<String>) {
    if let Some(stop_reason) = runtime.agent_stop_reason() {
        let state = stop_reason_state(&stop_reason);
        let note = (state != AgentTaskState::Completed).then(|| {
            format!(
                "The agent stopped with stop reason '{}'.",
                excerpt(&stop_reason, MAX_STOP_REASON_IN_NOTE)
            )
        });
        return (state, note);
    }
    match &session.state {
        SessionState::Ended { code: Some(0), .. } => (AgentTaskState::Completed, None),
        SessionState::Ended { code, .. } => (
            AgentTaskState::Failed,
            Some(match code {
                Some(code) => format!("The agent process exited with code {code}."),
                None => "The agent process exited without reporting a status.".to_string(),
            }),
        ),
        SessionState::Recovered { .. } => (
            AgentTaskState::Canceled,
            Some("The daemon that owned this agent died before it reported.".to_string()),
        ),
        SessionState::Live { .. } | SessionState::Silent { .. } => (AgentTaskState::Canceled, None),
    }
}

/// The A2A state one provider's stop reason means (`S5` decision 8, audit
/// S5-07).
///
/// The words the providers actually use, measured in this tree:
///
/// * `end_turn` — ACP's normal stop (`acp_client.rs`, `claude_view.rs` default)
///   and the Claude stream's own `stop_reason`;
/// * `completed` — codex's turn status (`codex_view.rs:849`);
/// * `interrupted` — codex's interrupted turn (`codex_view.rs:857`, `:1144`);
/// * `cancelled` / `canceled` — the daemon's own cancel path and ACP's
///   `cancelled`;
/// * anything else — `max_tokens`, `max_turn_requests`, `refusal`, pi's
///   `unknown` default (`pi_view.rs:165`), a reason from a provider version
///   this daemon has never seen — is `failed`. Failing closed is the point: a
///   creator that reads `completed` will believe work happened.
pub(super) fn stop_reason_state(stop_reason: &str) -> AgentTaskState {
    match stop_reason {
        "end_turn" | "completed" => AgentTaskState::Completed,
        "interrupted" | "cancelled" | "canceled" => AgentTaskState::Canceled,
        _ => AgentTaskState::Failed,
    }
}

/// How long a `stop_reason` may be in the note's excerpt (`S5` decision 8,
/// audit S5-13).
///
/// The reason is provider data and may be any string; the note is prose a human
/// reads, and the whole envelope is bounded, so the excerpt is cut here rather
/// than trusted to be small.
pub(super) const MAX_STOP_REASON_IN_NOTE: usize = 64;

/// The whole finish text's bound (`S5` decision 7, audit S5-13).
///
/// The summary is 4000 characters at most and the note is bounded, so this is
/// the ceiling the assembled envelope cannot pass: it is what keeps a finish
/// report deliverable at all, because the send path refuses an input larger
/// than its own write cap and a refused report is a creator that never learns
/// its child ended.
pub(crate) const MAX_FINISH_ENVELOPE_CHARS: usize = 8192;

/// The first `limit` characters of `text`, with an ellipsis when it was cut.
///
/// Characters, not bytes: a provider's reason may be any UTF-8, and cutting a
/// multi-byte sequence in half would panic rather than truncate.
pub(super) fn excerpt(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The envelope a creation's finish arrives in, bounded as a whole (`S5`
/// decision 7, audit S5-13).
///
/// The body is assembled once and then cut to
/// [`MAX_FINISH_ENVELOPE_CHARS`]; the artifact array is daemon-composed and
/// small, so the only field that can make this long is the summary (already cut
/// to `FINISH_SUMMARY_CHARS` by `summarise_message`) and the note, and cutting
/// the assembled text here is what keeps it deliverable at all.
pub(super) fn bound_finish_envelope(envelope: String) -> String {
    excerpt(&envelope, MAX_FINISH_ENVELOPE_CHARS)
}

/// The envelope one agent's message arrives in (S4-04).
///
/// The envelope is *prose for a model*, not a parser boundary: nothing on this
/// daemon's side reads it back, and the receiving agent is asked to treat it as
/// a note about who is speaking. That is exactly why the sender's own text must
/// not be able to write the daemon's delimiters: see
/// [`neutralise_envelope_text`]. `origin`, `role` and `from_agent` are composed
/// from daemon state (the caller's authenticated connection and a session id
/// validated either by its local row or by `validate_session_id`), never from
/// the message text. A local sender is rendered with its local id; a remote
/// sender is rendered as `peer:<authenticated-device>/<validated-far-id>` so
/// it cannot collide with the local form.
pub(super) fn agent_message_envelope(
    origin: &str,
    role: &str,
    from_agent: &str,
    text: &str,
) -> String {
    format!(
        "<devboule-system>\norigin: {origin}\nrole: {role}\nfrom_agent: {from_agent}\ntimestamp: {}\n{}\n</devboule-system>",
        unix_millis(),
        neutralise_envelope_text(text)
    )
}

/// Make the sender's text unable to close or reopen the envelope: every
/// case-insensitive occurrence of `<devboule-system` or `</devboule-system` is
/// escaped to `&lt;devboule-system`, and CRLF/CR are normalised to LF first so
/// the escaped text cannot smuggle a carriage return past the line the envelope
/// writes it on.
///
/// The excerpt fence markers are neutralised here too — **one rule, one
/// place**. The `agent_permission_request` frame quotes the child's words
/// between the exact lines `child-said:` and `end child-said`, and a child
/// that writes a line `end child-said` inside its own words would close its
/// quoted block early: the human would see less of the card than the model
/// does, with no marker that anything was cut. Any line that is exactly a
/// fence marker has its first scalar entity-escaped — the same escape the
/// tags get, applied at the marker's first character (`child-said:` becomes
/// `&#99;hild-said:`), which the app's exact-line parser can no longer match.
/// The app cannot tell an injected closer from a real one, which is why the
/// cure has to be here.
///
/// Escaping rather than stripping: the text still reads the way its author
/// wrote it, minus the delimiter it was trying to be. `pub(crate)` because
/// the peer-roster boundary (`mcp_peer_agents`) composes the same two passes
/// the card path composes rather than growing a second neutraliser.
pub(crate) fn neutralise_envelope_text(text: &str) -> String {
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut neutral = String::with_capacity(normalised.len());
    let mut cursor = 0;
    while let Some((start, len)) = next_envelope_delimiter(&normalised, cursor) {
        neutral.push_str(&normalised[cursor..start]);
        neutral.push_str("&lt;");
        neutral.push_str(&normalised[start + 1..start + len]);
        cursor = start + len;
    }
    neutral.push_str(&normalised[cursor..]);
    neutralise_excerpt_fences(&neutral)
}

/// The exact lines a `child-said:` fence is made of, and the entity escape of
/// each one's first scalar. Exact, never trimmed — the app's parser matches
/// the exact line only, so a padded or tabbed fence line is the child's own
/// text and is left alone here too.
const EXCERPT_FENCE_MARKERS: [(&str, &str); 2] = [
    ("child-said:", "&#99;hild-said:"),
    ("end child-said", "&#101;nd child-said"),
];

/// Escape any line that is exactly a fence marker, after the tag pass. Line
/// scoped, because the fence is line scoped: a marker buried inside a line is
/// words, not structure. The walk keeps every line ending byte-for-byte —
/// only the marker line's leading scalar changes.
fn neutralise_excerpt_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut touched = false;
    for segment in text.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let escaped = EXCERPT_FENCE_MARKERS
            .iter()
            .find(|(marker, _)| line == *marker)
            .map(|(_, escaped)| *escaped);
        match escaped {
            Some(escaped) => {
                out.push_str(escaped);
                if segment.ends_with('\n') {
                    out.push('\n');
                }
                touched = true;
            }
            None => out.push_str(segment),
        }
    }
    if touched {
        out
    } else {
        text.to_string()
    }
}

/// Byte offset and length of the next envelope delimiter at or after `from`,
/// compared case-insensitively. Both tags are scanned for, in one pass: the
/// closing tag does not contain the opening one character for character, so a
/// search for the opening tag alone would miss it.
fn next_envelope_delimiter(text: &str, from: usize) -> Option<(usize, usize)> {
    const OPEN: &[u8] = b"<devboule-system";
    const CLOSE: &[u8] = b"</devboule-system";
    let bytes = text.as_bytes();
    for start in from..bytes.len() {
        if bytes[start] != b'<' {
            continue;
        }
        for tag in [OPEN, CLOSE] {
            if bytes.len() - start >= tag.len()
                && bytes[start..start + tag.len()].eq_ignore_ascii_case(tag)
            {
                return Some((start, tag.len()));
            }
        }
    }
    None
}

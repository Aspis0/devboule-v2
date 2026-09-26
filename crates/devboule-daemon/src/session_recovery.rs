//! The road a session takes when its provider cannot be asked to reopen it: a
//! **new** session, same family, carrying the conversation read back from the
//! daemon's own journal as the preamble of its first prompt.
//!
//! Measured 2026-09-21 on a real session of the app: `session/load` for a
//! session whose working directory had been deleted answers in 42 ms with
//! `{"code":-32602,"message":"Invalid params: `cwd` does not exist on the
//! machine running the agent: …"}`. That is the provider's own refusal about a
//! fact the daemon can check without asking anyone (`session_resume.rs`, the
//! pre-flight) — and when the folder is gone, no provider call can bring the
//! session back at all. This module is the road that keeps the conversation:
//! the old row stays exactly as it is, and what the human said is handed to a
//! session that can start.
//!
//! The renderer here is deliberately the **coarse** one: user and agent text,
//! no tool cards, no thoughts. The readable transcript the panel draws is the
//! frontend's (`src/lib/agentSession.ts` `handleEvent`, `TranscriptItem`s in
//! `agentHost.ts`) and daemon code cannot call it; what the *agent* needs is
//! what the two sides said, and this file is the whole of that.

use super::*;
use devboule_protocol::NoticeSeverity;

/// How much of a recovered conversation travels into the new session's first
/// prompt.
///
/// 32 KiB is the daemon's existing cap for a prompt it composes by itself
/// (`crate::mcp_broker::tools::creation::request::MAX_AGENT_PROMPT_BYTES`), so a recovery is bounded by a
/// number this codebase already answered; the human's own prompt keeps its own
/// `MAX_WRITE_BYTES` of 64 KiB beside it. Eight thousand tokens of history is
/// what a person reads back in a reopened session, and everything older is
/// declared missing rather than silently dropped.
pub(super) const RECOVERED_CONTEXT_BYTES: usize = 32 * 1024;

/// The first phrase of every recovered context, and the one a test asserts on:
/// a prompt that came from the journal never pretends the human typed it.
pub(super) const RECOVERED_HEADER: &str = "Recovered conversation";

/// The phrase that says the context is **incomplete**. Present if and only if
/// turns were dropped, so its absence is a claim too.
pub(super) const RECOVERED_CUT_MARKER: &str = "the oldest turns were omitted";

/// One side of the conversation, as the renderer sees it. `id` is the
/// message the provider chunked: two events carrying the same id are one turn,
/// and two events with no id are two — a peer that does not name its messages
/// gets one turn per frame rather than a single block nobody can budget for.
struct Turn {
    role: &'static str,
    id: Option<String>,
    text: String,
}

/// The recovered conversation and what happened to it on the way in.
pub(super) struct RecoveredContext {
    /// The whole preamble text, header included.
    pub(super) text: String,
    /// How many turns the header says were kept.
    pub(super) kept: usize,
    /// How many the budget dropped, from the oldest end.
    pub(super) dropped: usize,
    /// Whether the one turn that survived is carried as its **tail**: nothing
    /// was dropped, and the text is still not the whole of what was said.
    pub(super) truncated: bool,
}

impl RecoveredContext {
    /// Whether the header declares the context incomplete, either way.
    pub(super) fn declares_a_cut(&self) -> bool {
        self.dropped > 0 || self.truncated
    }
}

/// The conversation `events` carry, oldest turn first.
///
/// The roles mirror what the panel shows as conversation (the frontend's
/// `handleAgentUserMessage`): the human's composer messages, both directions of
/// an agent-to-agent exchange, and the agent's own text. A daemon notice, a
/// creation stamp, a thought or an error is not something either side *said*,
/// so it is not part of what the agent is handed back: a recovery that quoted
/// the daemon to itself would be a worse reconstruction than a shorter one.
fn conversation_turns(events: &[SessionEvent]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for event in events {
        let (role, id, text) = match event {
            SessionEvent::AgentUserMessage {
                message_id,
                text,
                message_kind,
                ..
            } => match message_kind {
                devboule_protocol::UserMessageKind::Composer
                | devboule_protocol::UserMessageKind::OutgoingA2a
                | devboule_protocol::UserMessageKind::IncomingA2a
                | devboule_protocol::UserMessageKind::Unknown => {
                    ("user", message_id.clone(), text.as_str())
                }
                devboule_protocol::UserMessageKind::SystemNotice
                | devboule_protocol::UserMessageKind::Creation => continue,
            },
            SessionEvent::AgentMessage {
                message_id, text, ..
            } => ("agent", message_id.clone(), text.as_str()),
            _ => continue,
        };
        if text.trim().is_empty() {
            continue;
        }
        // Chunks of one message arrive as separate events carrying the same
        // id; consecutive text from the same side with the same id is one
        // turn, which is what a reader expects and what keeps the header's
        // turn count the count of things that were said.
        match turns.last_mut() {
            Some(last) if last.role == role && last.id.is_some() && last.id == id => {
                last.text.push_str(text);
            }
            _ => turns.push(Turn {
                role,
                id,
                text: text.to_string(),
            }),
        }
    }
    turns
}

fn rendered(turn: &Turn) -> String {
    format!("{}: {}", turn.role, turn.text.trim_end())
}

/// The newest `budget` bytes of one turn, on a character boundary, marked.
fn truncate_tail(text: &str, budget: usize) -> String {
    if text.len() <= budget {
        return text.to_string();
    }
    let mark = '…';
    let keep = budget.saturating_sub(mark.len_utf8());
    let mut start = text.len() - keep;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{mark}{}", &text[start..])
}

/// The context the new session's first prompt carries, or `None` when there is
/// no conversation to recover — a session nothing was ever said in, whose
/// reopening is refused in the daemon's own words instead of being replaced by
/// an empty new one.
pub(super) fn recovered_context(
    session_id: &str,
    reason: &str,
    events: &[SessionEvent],
    budget: usize,
) -> Option<RecoveredContext> {
    let turns = conversation_turns(events);
    if turns.is_empty() {
        return None;
    }
    let all: Vec<String> = turns.iter().map(rendered).collect();
    // Walk backwards from the newest turn: the oldest are the ones a budget
    // drops, and what survives is a suffix of the conversation.
    let mut kept_from = all.len();
    let mut used = 0usize;
    for (index, line) in all.iter().enumerate().rev() {
        let cost = line.len() + 2;
        if used + cost > budget {
            break;
        }
        used += cost;
        kept_from = index;
    }
    let body = if kept_from == all.len() {
        // Not even the newest turn fits the budget: the tail of it is still
        // the newest thing that was said, so the context carries that rather
        // than nothing — and says so, because a silent cut is the defect this
        // module exists to avoid.
        truncate_tail(&all[all.len() - 1], budget)
    } else {
        all[kept_from..].join("\n\n")
    };
    let truncated = kept_from == all.len();
    let dropped = if truncated { 0 } else { kept_from };
    let kept = all.len() - dropped;
    let mut header = format!("{RECOVERED_HEADER} ({kept} of {} turns", all.len());
    if dropped > 0 {
        header.push_str(&format!(
            "; {RECOVERED_CUT_MARKER} to fit this session's context budget of {budget} bytes"
        ));
    } else if truncated {
        header.push_str(&format!(
            "; it is longer than this session's context budget of {budget} bytes, so only its \
             end is carried — {RECOVERED_CUT_MARKER}"
        ));
    }
    header.push_str(").");
    let text = format!(
        "{header}\nRead back from this machine's own journal of session {session_id}, which \
         could not be reopened: {reason}\nThis is context the daemon attaches to this session's \
         first prompt, not a message the human just wrote. Continue the work from it.\n\n{body}"
    );
    Some(RecoveredContext {
        text,
        kept,
        dropped,
        truncated,
    })
}

/// The preamble a first prompt carries: the caller's own preset where it has
/// one, then the recovered conversation. Both are daemon text the human did not
/// write, so the warning `compose_first_prompt` states holds for either of
/// them, and `None` here means the caller has neither — in which case the
/// prompt travels byte for byte as it always did.
pub(super) fn preamble_with_recovered(
    preset: Option<&str>,
    recovered: Option<&str>,
) -> Option<String> {
    match (preset, recovered) {
        (None, None) => None,
        (Some(preset), None) => Some(preset.to_string()),
        (None, Some(recovered)) => Some(recovered.to_string()),
        (Some(preset), Some(recovered)) => Some(format!("{preset}\n\n{recovered}")),
    }
}

impl SessionRegistry {
    /// The session a session that cannot be reopened is replaced by.
    ///
    /// Every birth fact the row carries goes with it (creator, depth, overlay,
    /// profile, context): the replacement is the same session under a new id,
    /// and a child that came back as a root would be the escalation the lineage
    /// columns exist to stop. The directory is the one fact that depends on
    /// what is still on disk: a workspace row can be deleted while its folder
    /// stays (`workspace_delete` never checks who points at it), and then the
    /// recorded `cwd` brings the conversation back to the place it worked.
    /// When even that is gone the new session has no folder of its own, and
    /// the notice names the path that is missing rather than leaving the human
    /// with a workspace id to guess about.
    pub(super) fn recover_session(
        &self,
        state: &Arc<ServerState>,
        record: &SessionRecord,
        reason: &WireError,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<Session, WireError> {
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        let replay = journal.replay(&record.id)?;
        let Some(context) = recovered_context(
            &record.id,
            &reason.message,
            &replay.events,
            RECOVERED_CONTEXT_BYTES,
        ) else {
            // Nothing was ever said in this session: there is no conversation
            // to hand on, and a new empty session would answer a question the
            // human did not ask. The daemon's own refusal — the folder, named —
            // is the honest answer.
            return Err(reason.clone());
        };
        let lineage = Self::resumed_lineage(Some(record))?;
        // The directory the old session was launched in is read back, and used
        // when it is still a directory: the road is taken for a workspace whose
        // row was deleted as much as for one whose folder is gone, and only the
        // second case has to start without it.
        let recorded_dir = record.cwd.as_deref().map(Path::new);
        let working_dir = recorded_dir.filter(|path| path.is_dir());
        let recovered_name = record
            .display_name
            .clone()
            .unwrap_or_else(|| record.title.clone());
        let env_provider = std::env::var("DEVBOULE_AGENT_PROVIDER").ok();
        let meta = SessionCreateMeta {
            cwd: working_dir.map(Path::to_path_buf),
            display_name: Some(format!("{recovered_name} (recovered)")),
            created_by: record.created_by.clone(),
            depth: lineage.depth,
            overlay: lineage.overlay,
            origin: Some(record.origin.clone()),
            profile_id: record.profile_id.clone(),
            labels: record.labels.clone(),
            context_id: Some(record.context()),
            ..SessionCreateMeta::default()
        };
        // The replacement is a live daemon-owned session like any other, so it
        // holds the idle-shutdown slot every create road takes: the wire create
        // road takes one in `server::sessions` before it calls the registry, and
        // an agent child's own road takes its own in
        // `create_session_for_agent`. This road is the registry's, so nobody
        // upstream has taken one for it — and a live child the daemon does not
        // count is a daemon that can idle-exit under it. Taken here, after every
        // refusal above, so a recovery that creates nothing leaves no slot.
        if !state.session_started() {
            return Err(WireError::new(
                ErrorCode::ShuttingDown,
                "daemon is shutting down",
            ));
        }
        let session = match self.create_with_provider_env(
            state,
            owner,
            None,
            record.kind.clone(),
            record.provider.clone(),
            crate::profile_delivery::ProfileDelivery::for_request(None),
            None,
            &conn.conn_peer,
            env_provider.as_deref(),
            &meta,
        ) {
            Ok(session) => session,
            // The slot goes back with the failure, the way the wire create road
            // gives it back in its own `Err` arm.
            Err(error) => {
                state.session_finished();
                return Err(error);
            }
        };
        let runtime = self.runtime_for_user(&session.id, owner, conn)?;
        // The notice comes first: the human is looking at the new session's
        // transcript, and the one thing it must say before anything else is
        // which session they did not get and why.
        let starts_in = match working_dir {
            Some(path) => RecoveredDir::Kept(path),
            None => RecoveredDir::Gone(recorded_dir),
        };
        runtime.publish_session_notice(
            recovered_notice(&record.id, &reason.message, &context, &starts_in),
            NoticeSeverity::Info,
        );
        runtime.set_recovered_context(context.text);
        Ok(session)
    }
}

/// Where the replacement session starts, for the one sentence the human reads.
enum RecoveredDir<'a> {
    /// The directory the row recorded is still there, and the new session is
    /// launched in it.
    Kept(&'a Path),
    /// Nothing of the old directory survives. The path, when the row recorded
    /// one, is what the human is told is gone.
    Gone(Option<&'a Path>),
}

impl RecoveredDir<'_> {
    /// The half of the notice that answers "and where does my new session
    /// work?". Said with a path a human can check, because the daemon's reason
    /// can name a workspace id instead (`it does not exist`) and nobody can
    /// look at a folder they were never told.
    fn sentence(&self) -> String {
        match self {
            Self::Kept(path) => format!(
                " It starts in the directory the old session was launched in: {}.",
                crate::workspace::plain_path(&path.to_string_lossy())
            ),
            Self::Gone(Some(path)) => format!(
                " It has no folder of its own to work in: {} is gone.",
                crate::workspace::plain_path(&path.to_string_lossy())
            ),
            Self::Gone(None) => " It has no folder of its own to work in: the one the old \
                 session used is gone, and this row does not record its path."
                .to_string(),
        }
    }
}

fn recovered_notice(
    session_id: &str,
    reason: &str,
    context: &RecoveredContext,
    dir: &RecoveredDir<'_>,
) -> String {
    let cut = if context.declares_a_cut() {
        format!(
            " {RECOVERED_CUT_MARKER} to fit the context budget ({} of {} turns travelled whole).",
            context.kept,
            context.kept + context.dropped
        )
    } else {
        String::new()
    };
    format!(
        "This session could not be reopened ({reason}), so a new one was started with the \
         conversation recovered from the journal: it travels with the first prompt.{cut}{} \
         The session that was clicked is '{session_id}'; it and its transcript are untouched.",
        dir.sentence()
    )
}

//! The first-use human card for write tools, shared by group name.
//!
//! One responsibility: remembering which sessions a human already approved
//! for a write group, and raising the approval card once each.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use devboule_protocol::{OwnerId, PermissionOption, SessionEvent, SessionOrigin};

use crate::mcp_broker::McpBroker;
use crate::server::ServerState;

/// Creating a workspace has its own permission mark.
pub(in crate::mcp_broker) const WORKSPACES_GROUP: &str = "workspaces";
/// Removing a workspace has a separate mark because it closes sessions and
/// removes an existing checkout.
pub(in crate::mcp_broker) const WORKSPACE_ARCHIVE_GROUP: &str = "workspace_archiving";
/// The three terminal-write groups: one per act, never one for all three.
/// Opening a shell, typing into one and killing one are different powers, so
/// a session allowed the first is still carded for the other two — each act
/// raises its own card and keeps its own mark.
pub(in crate::mcp_broker) const TERMINAL_CREATE_GROUP: &str = "terminal_creation";
/// See [`TERMINAL_CREATE_GROUP`].
pub(in crate::mcp_broker) const TERMINAL_KEYS_GROUP: &str = "terminal_keys";
/// See [`TERMINAL_CREATE_GROUP`].
pub(in crate::mcp_broker) const TERMINAL_KILL_GROUP: &str = "terminal_kill";
/// Stopping a session's processes: its own mark, spent after each call
/// (the cleanup tool resets it), so an asking mode cards every time.
pub(in crate::mcp_broker) const PROCESS_CLEANUP_GROUP: &str = "process_cleanup";

/// Every first-use group's label, one table for all of them: the approval
/// card's button and its sentence both read from here, so a group id — the
/// underscore-joined token the marks are keyed by — never reaches the person.
/// A new group belongs in this table; what forgets its entry still renders
/// as words ([`group_label`]), never as the id.
const FIRST_USE_GROUP_LABELS: &[(&str, &str)] = &[
    (WORKSPACES_GROUP, "workspaces"),
    (WORKSPACE_ARCHIVE_GROUP, "workspace archiving"),
    (TERMINAL_CREATE_GROUP, "creating terminals"),
    (TERMINAL_KEYS_GROUP, "typing into terminals"),
    (TERMINAL_KILL_GROUP, "closing terminals"),
    (PROCESS_CLEANUP_GROUP, "cleaning up processes"),
];

/// The card choice that approves only the call it was raised for.
const CHOICE_ONCE: &str = "once";
/// The card choice that approves the group for the calling session. Its
/// kind is the journal's session word, so the ledger tells it from a
/// one-shot; the app renders that word as durable.
const SESSION_KIND: &str = "allow_session";
const CHOICE_SESSION: &str = "session";

/// Whether a session may call a write group without asking again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::mcp_broker) enum GateMark {
    /// No grant, and none asked for: the next call raises the card.
    None,
    /// A card for it is out and unanswered. A second call refuses as pending
    /// rather than spending the person's attention twice.
    Pending,
    /// Granted for the rest of this session.
    Open,
}

#[derive(Default)]
pub(in crate::mcp_broker) struct FirstUseGates {
    marks: Mutex<HashMap<(String, String), GateMark>>,
}

/// What claiming a set of groups found.
pub(in crate::mcp_broker) enum Claim {
    /// The group at this index is already granted for the rest of the session.
    Granted(usize),
    /// A card for these groups is out and unanswered.
    Pending,
    /// Nothing was granted or pending: every group is now pending, and the
    /// caller is the one to raise the card.
    Raised,
}

impl McpBroker {
    /// Look at a set of groups for one session and, when none of them is
    /// granted or has a card out, mark all of them pending: one look and one
    /// write under one lock, so two calls racing the same question cannot both
    /// be the one that raises the card.
    pub(in crate::mcp_broker) fn claim_gate_marks(
        &self,
        session_id: &str,
        groups: &[String],
    ) -> Claim {
        let Ok(mut marks) = self.write_gates.marks.lock() else {
            return Claim::Pending;
        };
        for (index, group) in groups.iter().enumerate() {
            match marks.get(&(session_id.to_string(), group.clone())) {
                Some(GateMark::Open) => return Claim::Granted(index),
                Some(GateMark::Pending) => return Claim::Pending,
                Some(GateMark::None) | None => {}
            }
        }
        for group in groups {
            marks.insert((session_id.to_string(), group.clone()), GateMark::Pending);
        }
        Claim::Raised
    }

    /// Remember `mark` for one session and group.
    pub(in crate::mcp_broker) fn remember_gate_mark(
        &self,
        session_id: &str,
        group: &str,
        mark: GateMark,
    ) {
        if let Ok(mut marks) = self.write_gates.marks.lock() {
            marks.insert((session_id.to_string(), group.to_string()), mark);
        }
    }

    /// The gate mark for one session and group, if any. Test-only:
    /// production reads the gate through `ensure_write_allowed` alone.
    #[cfg(test)]
    pub(in crate::mcp_broker) fn first_use_mark(
        &self,
        session_id: &str,
        group: &str,
    ) -> Option<GateMark> {
        self.write_gates
            .marks
            .lock()
            .ok()?
            .get(&(session_id.to_string(), group.to_string()))
            .copied()
    }

    /// The gate's mark table, held: whoever must read or write a mark waits
    /// until it is dropped, which is how a test stops a call at a known point.
    #[cfg(test)]
    pub(in crate::mcp_broker) fn hold_gate_marks_for_test(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<(String, String), GateMark>> {
        self.write_gates.marks.lock().expect("first-use gate lock")
    }

    #[cfg(test)]
    pub(in crate::mcp_broker) fn open_first_use_gate_for_test(
        &self,
        session_id: &str,
        group: &str,
    ) {
        self.write_gates
            .marks
            .lock()
            .expect("first-use gate lock")
            .insert((session_id.to_string(), group.to_string()), GateMark::Open);
    }

    /// Forget every mark of one session: called when its bearer goes away,
    /// so a later session never inherits an approval it did not ask for.
    pub(in crate::mcp_broker) fn forget_first_use(&self, session_id: &str) {
        if let Ok(mut marks) = self.write_gates.marks.lock() {
            marks.retain(|(session, _), _| session != session_id);
        }
    }
}

/// Pass the write gate for `group`, raising the human card on the caller's
/// own session the first time the session's mode asks.
///
/// `subject` says what this call is about ("create workspace 'Desk'") and
/// `facts` are the `key: value` lines the card shows under it. Both travel
/// on the card because a licence without them asks the person to approve
/// what they cannot see.
///
/// The calling session's current mode is read at call time — it can change
/// mid-session, so it is never cached. An automatic mode proceeds with no
/// card and sets no mark, so switching back to an asking mode asks again; a
/// plan or read-only mode is refused before the card, naming the mode.
///
/// The card is a chooser — two allow options of one kind — so the app shows
/// both choices by name and the delegation door refuses it the way it
/// refuses every question: only a person answers. "Allow this call"
/// proceeds without opening the group; "Allow for this session" opens it.
/// Denied, timed out or otherwise unanswered refuses the call and leaves
/// the gate shut, so the next call asks again. A second call racing the
/// first is refused as pending rather than raising a second card.
pub(in crate::mcp_broker) fn ensure_write_allowed(
    state: &ServerState,
    broker: &McpBroker,
    session_id: &str,
    owner: &OwnerId,
    group: &str,
    subject: &str,
    facts: &[(&str, &str)],
) -> Result<(), String> {
    ensure_write_approved(state, broker, session_id, owner, group, subject, facts).map(|_| ())
}

/// Who approved a write that passed the gate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::mcp_broker) enum Approval {
    /// The session's automatic mode: no card was raised.
    Mode,
    /// A person: a card answered now, or a session grant a card gave earlier.
    Person,
}

/// [`ensure_write_allowed`] naming who approved, for a caller whose audit
/// row must say that an automatic mode did.
pub(in crate::mcp_broker) fn ensure_write_approved(
    state: &ServerState,
    broker: &McpBroker,
    session_id: &str,
    owner: &OwnerId,
    group: &str,
    subject: &str,
    facts: &[(&str, &str)],
) -> Result<Approval, String> {
    // The calling session's mode, read now: it can change
    // mid-session, so it is never cached and never stored as a mark. An
    // automatic mode proceeds with no card; a plan or read-only mode of the
    // session's own family is refused before the card, naming the mode;
    // anything else cards.
    let gate = state
        .sessions
        .live_runtime(session_id, owner)
        .map(|runtime| runtime.mode_gate());
    match gate {
        Some(crate::provider_catalog::ModeGate::Auto) => return Ok(Approval::Mode),
        Some(crate::provider_catalog::ModeGate::Refuse(sentence)) => return Err(sentence),
        _ => {}
    }
    {
        let mut marks = broker
            .write_gates
            .marks
            .lock()
            .map_err(|_| "MCP state is unavailable.".to_string())?;
        match marks.get(&(session_id.to_string(), group.to_string())) {
            Some(GateMark::Open) => return Ok(Approval::Person),
            Some(GateMark::Pending) => {
                return Err("permission pending; retry".to_string());
            }
            None | Some(GateMark::None) => {
                marks.insert(
                    (session_id.to_string(), group.to_string()),
                    GateMark::Pending,
                );
            }
        }
    }
    let card = write_gate_card(session_id, group, subject, facts);
    let card_id = gate_card_id(&card).to_string();
    let outcome = request_card(state, session_id, owner, &card_id, card);
    let mut marks = broker
        .write_gates
        .marks
        .lock()
        .map_err(|_| "MCP state is unavailable.".to_string())?;
    let key = (session_id.to_string(), group.to_string());
    // Marks move only out of Pending: an answer to a card the gate already
    // forgot (a re-registration cleared it) must neither open the group nor
    // clear a newer grant.
    let pending = marks.get(&key) == Some(&GateMark::Pending);
    match outcome {
        Answered::Chose(choice) if pending && choice == CHOICE_SESSION => {
            marks.insert(key, GateMark::Open);
            Ok(Approval::Person)
        }
        Answered::Chose(choice) if pending && choice == CHOICE_ONCE => {
            marks.remove(&key);
            Ok(Approval::Person)
        }
        Answered::Chose(choice) if choice == "deny" => {
            if pending {
                marks.remove(&key);
            }
            Err("permission refused".to_string())
        }
        Answered::Undelivered => {
            if pending {
                marks.remove(&key);
            }
            Err("permission card could not be delivered".to_string())
        }
        _ => {
            if pending {
                marks.remove(&key);
            }
            Err("permission request was not answered".to_string())
        }
    }
}

/// The card the gate raises, and the person's answer to it.
fn request_card(
    state: &ServerState,
    session_id: &str,
    owner: &OwnerId,
    card_id: &str,
    card: SessionEvent,
) -> Answered {
    let Some(runtime) = state.sessions.live_runtime(session_id, owner) else {
        return Answered::Undelivered;
    };
    let Some(card_broker) = runtime.permission_broker() else {
        return Answered::Undelivered;
    };
    card_broker.watch_card_choice(card_id);
    let decision = state
        .sessions
        .ask_creation_card_decision(session_id, owner, card);
    let choice = card_broker.take_card_choice(card_id).flatten();
    match decision {
        None | Some(crate::session::HostCardDecision::Cancelled) => Answered::Undelivered,
        Some(crate::session::HostCardDecision::Deny) => Answered::Chose("deny".to_string()),
        Some(crate::session::HostCardDecision::Timeout) => Answered::Unanswered,
        Some(crate::session::HostCardDecision::Allow) => {
            choice.map_or(Answered::Unanswered, Answered::Chose)
        }
    }
}

/// The answer to a card, before the gate reads it as one of its own choices:
/// the option the person picked by id, or that the card was never delivered or
/// never answered.
enum Answered {
    Chose(String),
    Undelivered,
    Unanswered,
}

/// Ask the person to allow this call, offering the card's own choices, and
/// hand back the option they chose.
///
/// The one place a card is raised without consulting the session's mode for
/// whether to raise it at all. An automatic mode proceeds without a card for
/// every write group, because there is nothing in a write the person has to
/// see; a saved-login fill has, so this card goes up however the session runs.
/// A plan or read-only mode is refused before the card, exactly as the write
/// gate refuses it: those modes do not act.
pub(in crate::mcp_broker) fn ask_choice(
    state: &ServerState,
    session_id: &str,
    owner: &OwnerId,
    card: SessionEvent,
) -> Result<String, String> {
    if let Some(crate::provider_catalog::ModeGate::Refuse(sentence)) = state
        .sessions
        .live_runtime(session_id, owner)
        .map(|runtime| runtime.mode_gate())
    {
        return Err(sentence);
    }
    let card_id = gate_card_id(&card).to_string();
    match request_card(state, session_id, owner, &card_id, card) {
        Answered::Chose(option) => Ok(option),
        Answered::Undelivered => Err("permission card could not be delivered".to_string()),
        Answered::Unanswered => Err("permission request was not answered".to_string()),
    }
}

/// The approval card: an ordinary permission request with no creation
/// payload. The broker stamps the caller's own origin on the way in, so the
/// card carries the unknown placeholder here, as the creation card does.
///
/// The card says it is a chooser so the app renders both choices by name;
/// the delegation door refuses first-use ids structurally, so the rendering
/// flag carries no safety weight. The session choice journals
/// under its own kind, which the app renders as durable.
fn write_gate_card(
    session_id: &str,
    group: &str,
    subject: &str,
    facts: &[(&str, &str)],
) -> SessionEvent {
    let listed = facts
        .iter()
        .map(|(key, value)| format!("{key}: {}", oneline(value)))
        .collect::<Vec<_>>()
        .join("\n");
    let subject = oneline(subject);
    let label = group_label(group);
    let session_label = format!("Allow {label} for this session");
    let description = if listed.is_empty() {
        format!(
            "An agent requested permission to {subject} for the first time. \
             \"Allow this call\" approves only this call. \
             \"{session_label}\" approves {label} from this session from now on."
        )
    } else {
        let marked = mark_fact_lines(&listed).join("\n");
        format!(
            "An agent requested permission to {subject} for the first time:\n{marked}\n\n\
             \"Allow this call\" approves only this call. \
             \"{session_label}\" approves {label} from this session from now on."
        )
    };
    SessionEvent::PermissionRequest {
        tool_call_id: card_id(session_id, group),
        title: format!("Approve request to {subject}"),
        description: Some(description),
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            PermissionOption {
                option_id: CHOICE_ONCE.to_string(),
                name: "Allow this call".to_string(),
                kind: "allow_once".to_string(),
            },
            PermissionOption {
                option_id: CHOICE_SESSION.to_string(),
                name: session_label.to_string(),
                kind: SESSION_KIND.to_string(),
            },
            PermissionOption {
                option_id: "deny".to_string(),
                name: "Deny".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
        // Forced: the two allow options differ in kind (the journal tells
        // them apart), so the stamp would not derive this on its own — and
        // without it the app shows only its generic pair and the session
        // choice is unclickable.
        is_chooser: Some(true),
        kind: None,
        plan: None,
        questions: None,
        origin: SessionOrigin::unknown(),
        create_agent: None,
    }
}

/// One group's label as the person reads it: the table above, and for a
/// group that forgot its entry the id spelled as words — a card must never
/// show `some_group` as a noun.
fn group_label(group: &str) -> String {
    FIRST_USE_GROUP_LABELS
        .iter()
        .find(|(name, _)| *name == group)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| group.replace('_', " "))
}

/// One line of card text with no surprises in it: every line break a
/// renderer may honour becomes a space, so a fact stays one fact.
fn oneline(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(char) = chars.next() {
        let cr_lf = char == '\r' && chars.peek() == Some(&'\n');
        if is_line_break(char) {
            out.push(' ');
            if cr_lf {
                chars.next();
            }
        } else {
            out.push(char);
        }
    }
    out
}

/// Fact lines the daemon vouches for: every line the value breaks into is
/// prefixed, so a forged terminator inside a fact is just another marked
/// line and the card's own sentences stay recognisable. The same break set
/// the creation card marks its prompt with.
fn mark_fact_lines(listed: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = listed.chars().peekable();
    while let Some(char) = chars.next() {
        let cr_lf = char == '\r' && chars.peek() == Some(&'\n');
        if is_line_break(char) {
            lines.push(format!("| {}", std::mem::take(&mut current)));
            if cr_lf {
                chars.next();
            }
        } else {
            current.push(char);
        }
    }
    lines.push(format!("| {current}"));
    lines
}

fn is_line_break(char: char) -> bool {
    matches!(
        char,
        '\n' | '\r' | '\u{2028}' | '\u{2029}' | '\u{0085}' | '\u{000B}' | '\u{000C}'
    )
}

/// The card id of a gate card just built, for the watch the gate sets
/// before raising it.
fn gate_card_id(card: &SessionEvent) -> &str {
    match card {
        SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id,
        _ => "",
    }
}

/// The correlation id of one gate card. Distinct per call, so two sessions
/// racing the same group cannot collide in the pending table.
pub(in crate::mcp_broker) fn card_id(session_id: &str, group: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "{}{group}:{session_id}:{:x}-{}",
        crate::mcp_broker::FIRST_USE_CARD_PREFIX,
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
#[path = "mcp_first_use_tests.rs"]
mod tests;

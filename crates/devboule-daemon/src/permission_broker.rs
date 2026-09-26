//! Shared permission broker for ACP and Claude stream-json sessions.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};

use devboule_protocol::{
    NoticeSeverity, PermissionOption, PermissionOutcome, PermissionRequestKind, SessionEvent,
    SessionOrigin, SessionOriginKind,
};

use super::SessionRuntime;

const MAX_PENDING_ACP_PERMISSIONS: usize = 32;
/// How many undecided permission cards one paired device may hold at once
/// across **every** session (`§8b A14`, H2).
///
/// A peer's session can raise one card per tool call, and every one of them
/// lands in the same queue the person at this machine reads. Three is the
/// design's number: enough for an agent's immediate steps, small enough that a
/// device cannot turn the desktop into its own approval prompt. The fourth is
/// refused, not stacked.
///
/// The count lives in [`peer_cards`], one map for the whole daemon, not in the
/// per-session table: a per-broker count gave a device three cards *per
/// session*, so opening a second session bought it three more, and the third
/// session nine. The scope of the promise is the device, so the scope of the
/// counter is the device.
const MAX_PENDING_FOR_PEER: usize = 3;

/// The undecided cards this daemon is holding, per authenticated origin device.
///
/// One map for the whole server (this process): the broker table is per
/// session, and the allowance is per device. The key is the **origin device id
/// this daemon stamped** on the request (`stamp_origin` writes the session's
/// stored origin; nothing a caller sends can reach it), which is the same id
/// `check_user_owner` scopes a `Daemon` peer by.
///
/// `OnceLock` rather than a field because the brokers are built inside the
/// provider clients: the counter has to exist before any of them, and one
/// process is one daemon with one card allowance.
fn peer_cards() -> &'static Mutex<HashMap<String, usize>> {
    static PEER_CARDS: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    PEER_CARDS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn peer_cards_lock() -> std::sync::MutexGuard<'static, HashMap<String, usize>> {
    peer_cards()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// Take one of `MAX_PENDING_FOR_PEER` slots for `device_id`. `false` when the
/// device is already holding all of them.
///
/// Reserved *before* the card is inserted, so the allowance cannot be
/// overrun by two sessions registering at the same moment.
fn reserve_peer_card(device_id: &str) -> bool {
    let mut cards = peer_cards_lock();
    let count = cards.entry(device_id.to_string()).or_insert(0);
    if *count >= MAX_PENDING_FOR_PEER {
        return false;
    }
    *count += 1;
    true
}

/// Give back one slot: the card this device was holding is decided, cancelled
/// or gone with its session.
fn release_peer_card(device_id: &str) {
    let mut cards = peer_cards_lock();
    if let Some(count) = cards.get_mut(device_id) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            cards.remove(device_id);
        }
    }
}

/// Forget every slot `device_id` holds. The peer's connections are gone, so
/// the cards it was holding can no longer be answered by it; the cards
/// themselves stay pending for the person at this machine.
pub(crate) fn release_peer_cards(device_id: &str) {
    peer_cards_lock().remove(device_id);
}

/// The slots `device_id` currently holds. Test-only: the count is the thing
/// the allowance is about, and a test that cannot read it can only assert
/// refusals.
#[cfg(test)]
pub(crate) fn peer_card_count(device_id: &str) -> usize {
    peer_cards_lock().get(device_id).copied().unwrap_or(0)
}

/// Release the slot one removed card was holding, when it was a peer's.
fn release_card_slot(request: &SessionEvent) {
    if let Some(device_id) = peer_origin_device(request) {
        release_peer_card(&device_id);
    }
}

pub(super) const MAX_ACP_PERMISSION_FIELD_BYTES: usize = 8 * 1024;
pub(super) const MAX_ACP_PERMISSION_OPTIONS: usize = 32;
/// How many questions one `question` request may carry. Claude's
/// `AskUserQuestion` holds at most four; eight leaves headroom without
/// letting one card become a questionnaire.
const MAX_QUESTIONS_PER_REQUEST: usize = 8;
const MAX_ACP_PERMISSION_ARGS: usize = 256;
const MAX_ACP_PERMISSION_ENV: usize = 64;

pub(super) type PermissionSender = dyn Fn(u64, serde_json::Value) -> io::Result<()> + Send + Sync;

/// How a pending permission is completed. Agent-initiated prompts write a
/// JSON-RPC result to the agent's stdin. Host-initiated prompts (the
/// `terminal/create` gate) wake the host RPC thread that is blocked on the
/// decision instead — they must not invent an ACP permission response.
enum PermissionResponder {
    Agent { acp_id: u64 },
    Host,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HostDecision {
    Allow,
    Deny,
    Timeout,
    Cancelled,
}

struct PermissionCompletion {
    done: bool,
    decision: Option<HostDecision>,
}

pub(super) struct PendingPermission {
    responder: PermissionResponder,
    tool_call_id: String,
    session_id: String,
    request: SessionEvent,
    runtime: std::sync::Weak<SessionRuntime>,
    done: Arc<(Mutex<PermissionCompletion>, std::sync::Condvar)>,
}

/// An allow option chosen for an unattended-mode request, with the pending
/// entry it decides.
struct AutoAnswer {
    pending: Arc<PendingPermission>,
    option: PermissionOption,
}

/// What a delegated-answer check saw when it looked a card up. `Found`
/// carries the pending entry so the later take can pin it (`Arc::ptr_eq`):
/// a card resolved between the check and the answer makes the take refuse,
/// not resolve.
pub(super) enum DelegatedPeek {
    Found {
        pending: Arc<PendingPermission>,
        options: Vec<PermissionOption>,
        is_question: bool,
    },
    Absent,
}

/// The gate value for the delegated answer path (`§4.2`).
///
/// No public constructor: the fields are private and the only constructor is
/// `checked_delegated`, a private function of this module, whose single call
/// site is at the end of [`PermissionBroker::answer_delegated`]'s check
/// sequence. A caller outside the sequence cannot mint one, so a delegated
/// answer cannot reach [`PermissionBroker::resolve_checked`] with a card the
/// checks did not read; and a check that fails returns before the value
/// exists, so it cannot be diverted into a resolution — not even the
/// cancelled kind the human path's unsupported-outcome branch takes.
pub(super) struct CheckedDelegatedCard {
    pending: Arc<PendingPermission>,
    option: PermissionOption,
    journal_outcome: &'static str,
    answered_by: String,
}

/// Private constructor: one call site, after the checks. Not `pub`, so the
/// promise above is compiler-enforced for the whole crate outside this file.
fn checked_delegated(
    pending: Arc<PendingPermission>,
    option: PermissionOption,
    journal_outcome: &'static str,
    answered_by: String,
) -> CheckedDelegatedCard {
    CheckedDelegatedCard {
        pending,
        option,
        journal_outcome,
        answered_by,
    }
}

struct PermissionTable {
    entries: HashMap<String, Arc<PendingPermission>>,
    closed: bool,
}

pub(crate) struct PermissionBroker {
    sender: Arc<PermissionSender>,
    pending: Mutex<PermissionTable>,
    require_journal: bool,
    /// Cards whose answer a first-use gate is waiting on. Watched only
    /// while a gate waits: the gate un-watches when its wait ends either
    /// way, so this never grows past the gates in flight.
    watch_choices: Mutex<HashSet<String>>,
    /// The option id each watched card was answered with, taken once by
    /// the gate that watched it. The journal keeps the grant kind; only
    /// the option id tells a one-shot allow from a session licence.
    choice_answers: Mutex<HashMap<String, Option<String>>>,
    #[cfg(test)]
    after_take_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

/// The transcript's one sentence for a refused repeat: an id whose decision
/// the journal already holds gets no second card. One plain sentence — the
/// person reading the transcript did not ask for protocol vocabulary.
const REUSED_ID_NOTICE: &str =
    "The agent reused the id of a question this session already closed, so this request was declined.";

#[derive(Debug)]
pub(super) enum PermissionResponseError {
    NotFound,
    InvalidRequest(String),
    /// The id's decision is already in the journal: `register_with` refused
    /// the card before it existed and put [`REUSED_ID_NOTICE`] up as the
    /// transcript's one notice. The caller sends its cancelled frame and
    /// adds no second message.
    AlreadyRecorded,
    Io(io::Error),
}

impl fmt::Display for PermissionResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("permission request is no longer pending"),
            Self::InvalidRequest(message) => formatter.write_str(message),
            Self::AlreadyRecorded => formatter.write_str(REUSED_ID_NOTICE),
            Self::Io(error) => write!(
                formatter,
                "could not answer ACP permission request: {error}"
            ),
        }
    }
}

impl PermissionBroker {
    #[cfg(test)]
    pub(super) fn for_test(sender: Arc<PermissionSender>) -> Arc<Self> {
        Arc::new(Self {
            sender,
            pending: Mutex::new(PermissionTable {
                entries: HashMap::new(),
                closed: false,
            }),
            require_journal: false,
            watch_choices: Mutex::new(HashSet::new()),
            choice_answers: Mutex::new(HashMap::new()),
            #[cfg(test)]
            after_take_hook: Mutex::new(None),
        })
    }

    pub(super) fn send(&self, id: u64, result: serde_json::Value) -> io::Result<()> {
        (self.sender)(id, result)
    }

    pub(super) fn with_sender(sender: Arc<PermissionSender>) -> Arc<Self> {
        Arc::new(Self {
            sender,
            pending: Mutex::new(PermissionTable {
                entries: HashMap::new(),
                closed: false,
            }),
            require_journal: true,
            watch_choices: Mutex::new(HashSet::new()),
            choice_answers: Mutex::new(HashMap::new()),
            #[cfg(test)]
            after_take_hook: Mutex::new(None),
        })
    }

    pub(super) fn register(
        &self,
        acp_id: u64,
        request: SessionEvent,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<Arc<PendingPermission>, PermissionResponseError> {
        self.register_with(PermissionResponder::Agent { acp_id }, request, runtime)
    }

    fn register_host(
        &self,
        request: SessionEvent,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<Arc<PendingPermission>, PermissionResponseError> {
        self.register_with(PermissionResponder::Host, request, runtime)
    }

    fn register_with(
        &self,
        responder: PermissionResponder,
        request: SessionEvent,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<Arc<PendingPermission>, PermissionResponseError> {
        // The origin and the verdict are stamped here, at the one point a
        // request becomes pending: the card renders a `peer` origin as its
        // own first line, and the request's own text must never be able to
        // imitate it (§8b A14). The verdict has to be here too — the stored
        // copy is what the audit row and the replay roads read (review A2a
        // #4). `publish_agent_event_with_seq` writes both again on the way
        // out, which is what covers publishers that never come through here.
        let request = stamp_origin(request, runtime.origin());
        let request = stamp_chooser(request);
        let tool_call_id = match &request {
            SessionEvent::PermissionRequest { tool_call_id, .. } => tool_call_id.clone(),
            _ => {
                return Err(PermissionResponseError::InvalidRequest(
                    "not a permission request".to_string(),
                ));
            }
        };
        validate_permission_request(&tool_call_id, &request)?;
        // An id the journal already holds a decision for is DONE: a second
        // card here would show a person an answer that can never be written
        // (the audit row is write-once), and the agent would be told
        // `cancelled` after the human spent the effort — the live P1
        // (review-A2a). Refused before the card exists, from the one road
        // every family registers through: no card, the family's own
        // cancelled frame, and [`REUSED_ID_NOTICE`] as the transcript's one
        // notice.
        if runtime.permission_already_recorded(&tool_call_id) {
            let _ =
                runtime.publish_session_notice(REUSED_ID_NOTICE.to_string(), NoticeSeverity::Info);
            return Err(PermissionResponseError::AlreadyRecorded);
        }
        let pending = Arc::new(PendingPermission {
            responder,
            tool_call_id: tool_call_id.clone(),
            session_id: runtime.session_id.clone(),
            request,
            runtime: Arc::downgrade(runtime),
            done: Arc::new((
                Mutex::new(PermissionCompletion {
                    done: false,
                    decision: None,
                }),
                std::sync::Condvar::new(),
            )),
        });
        let mut table = self
            .pending
            .lock()
            .map_err(|_| io_error("permission broker lock poisoned"))?;
        if table.closed {
            return Err(PermissionResponseError::InvalidRequest(
                "permission broker is closed".to_string(),
            ));
        }
        if table.entries.contains_key(&tool_call_id) {
            return Err(PermissionResponseError::InvalidRequest(format!(
                "permission request {tool_call_id} is already pending"
            )));
        }
        // One paired device may hold at most `MAX_PENDING_FOR_PEER` undecided
        // cards, across every session it owns: the queue a peer fills is the
        // queue the person at this machine has to read. The slot is taken from
        // the daemon-wide counter before the card exists, and the counter is
        // keyed by the origin device this request was stamped with, so two
        // devices' sessions never share the allowance and two sessions of one
        // device never multiply it (H2).
        if let Some(device_id) = peer_origin_device(&pending.request) {
            if !reserve_peer_card(&device_id) {
                return Err(PermissionResponseError::InvalidRequest(format!(
                    "that device already has {MAX_PENDING_FOR_PEER} permission requests waiting"
                )));
            }
        }
        let session_pending = table
            .entries
            .values()
            .filter(|pending| pending.session_id == runtime.session_id)
            .count();
        if session_pending >= MAX_PENDING_ACP_PERMISSIONS {
            // The slot taken above belongs to a card that will not exist.
            release_card_slot(&pending.request);
            return Err(PermissionResponseError::InvalidRequest(format!(
                "session has reached the maximum of {MAX_PENDING_ACP_PERMISSIONS} pending permission requests"
            )));
        }
        table.entries.insert(tool_call_id, Arc::clone(&pending));
        drop(table);
        // The card is now parked: the one moment a per-card observer may
        // learn about it (the delegated-surfacing hook the registry installs
        // at birth). Fired after the lock is dropped, so the observer sees a
        // consistent table and cannot re-enter it.
        runtime.notify_permission_park(&pending.request);
        // The status this session publishes is "a card awaits an answer" from
        // this moment. The event that carried the request here was published
        // before the card existed, so the publisher's own check could not have
        // seen it: this is the point the fact becomes true.
        runtime.publish_activity_change();
        Ok(pending)
    }

    pub(super) fn respond(
        &self,
        tool_call_id: &str,
        outcome: PermissionOutcome,
    ) -> Result<(), PermissionResponseError> {
        self.respond_with_option(tool_call_id, outcome, None, None)
    }

    pub(super) fn respond_with_option(
        &self,
        tool_call_id: &str,
        outcome: PermissionOutcome,
        option_id: Option<String>,
        answer: Option<String>,
    ) -> Result<(), PermissionResponseError> {
        let (options, is_question) = {
            let table = self
                .pending
                .lock()
                .map_err(|_| io_error("permission broker lock poisoned"))?;
            let pending = table
                .entries
                .get(tool_call_id)
                .ok_or(PermissionResponseError::NotFound)?;
            match &pending.request {
                SessionEvent::PermissionRequest { options, kind, .. } => (
                    options.clone(),
                    matches!(kind, Some(PermissionRequestKind::Question)),
                ),
                _ => (Vec::new(), false),
            }
        };
        // A question's free-text answer travels beside the option pick,
        // never in it: one carrier per answer, and a grant names what the
        // person chose — the implicit first-option pick below would answer
        // what was never picked.
        if is_question {
            if option_id.is_some() && answer.is_some() {
                return Err(PermissionResponseError::InvalidRequest(
                    "a question answer names an option or carries text, not both".to_string(),
                ));
            }
            if let Some(answer) = answer {
                if outcome != PermissionOutcome::AllowOnce {
                    return Err(PermissionResponseError::InvalidRequest(
                        "a question's text answer must grant, not refuse".to_string(),
                    ));
                }
                validate_permission_field("answer", &answer)?;
                let pending = self.take(tool_call_id, None)?;
                #[cfg(test)]
                self.run_after_take_hook();
                // The text itself goes only to the provider's reply frame
                // below: the `permissions` row records the request plus
                // `allow_once`, and the resolved event carries the granted
                // kind with no option — the person's own words stay out of
                // this store, the logs, and the peer audit, the way a chat
                // message's words would.
                return self.complete(
                    &pending,
                    serde_json::json!({
                        "outcome": { "outcome": "selected", "answer": answer }
                    }),
                    None,
                    "allow_once",
                    None,
                );
            }
            if outcome == PermissionOutcome::Deny && option_id.is_none() {
                // Dismissing the question card: a refusal with nothing to
                // name. Always an answer — including for a single-option
                // question, which is not a chooser and would otherwise
                // report the completed cancel as an error.
                let pending = self.take(tool_call_id, None)?;
                #[cfg(test)]
                self.run_after_take_hook();
                return self.complete(
                    &pending,
                    serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
                    None,
                    "cancelled",
                    None,
                );
            }
            if outcome == PermissionOutcome::AllowOnce && option_id.is_none() {
                return Err(PermissionResponseError::InvalidRequest(
                    "a question grant must name the picked option or carry its text".to_string(),
                ));
            }
        } else if answer.is_some() {
            return Err(PermissionResponseError::InvalidRequest(
                "only a question request carries a text answer".to_string(),
            ));
        }
        let option = select_option(&options, outcome, option_id.as_deref())
            .map_err(PermissionResponseError::InvalidRequest)?;
        let pending = self.take(tool_call_id, None)?;
        #[cfg(test)]
        self.run_after_take_hook();
        let Some(option) = option else {
            let completed = self.complete(
                &pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
                None,
                "cancelled",
                None,
            );
            return match outcome {
                // A deny on a CHOOSER with nothing to name: ACP's only
                // refusal for this request is a cancellation, and the
                // daemon delivered one — an answer, so the card reads
                // Denied, not an error and not the queue again (review A2a
                // #3). Every other unhonored outcome keeps the sentence
                // saying what the daemon did: the ordinary card disables
                // Deny whenever the request offers no reject option, so a
                // chooser's enabled Deny is the only door into this arm.
                PermissionOutcome::Deny if options_form_a_chooser(&options) => completed,
                _ => match completed {
                    Ok(()) => Err(PermissionResponseError::InvalidRequest(
                        unsupported_outcome_reason(&options, outcome),
                    )),
                    Err(error) => Err(error),
                },
            };
        };
        // Only the exact one-shot kind is resolved implicitly; a durable
        // option stays pending until the client names it.
        let result = serde_json::json!({
            "outcome": { "outcome": "selected", "optionId": option.option_id }
        });
        self.complete(
            &pending,
            result,
            Some(&option),
            match outcome {
                // The journal records the kind that was really granted,
                // never a one-shot constant (auto_answer's rule) — in the
                // journal's own vocabulary, where the durable allow words
                // are `allow_always` and the first-use gate's `allow_session`.
                // An open allow kind outside that vocabulary falls back to
                // the posted outcome rather than a word
                // `decision_from_outcome` would read as cancelled and deny
                // a grant that happened.
                PermissionOutcome::AllowOnce if option.kind == "allow_always" => "allow_always",
                PermissionOutcome::AllowOnce if option.kind == "allow_session" => "allow_session",
                PermissionOutcome::AllowOnce => "allow_once",
                PermissionOutcome::Deny => "deny",
            },
            // The human wire path: no attribution, by definition.
            None,
        )
    }

    /// Read one pending card's own options, **without** touching it — the
    /// check the cancel-trap demands (§0.3: `respond_with_option` with an
    /// outcome the card does not support completes the card as cancelled, so
    /// the delegated path validates against the entry before any `respond*`
    /// call).
    pub(super) fn peek_delegated(&self, tool_call_id: &str) -> DelegatedPeek {
        let Ok(table) = self.pending.lock() else {
            return DelegatedPeek::Absent;
        };
        match table.entries.get(tool_call_id) {
            Some(pending) => {
                let (options, is_question) = match &pending.request {
                    SessionEvent::PermissionRequest { options, kind, .. } => (
                        options.clone(),
                        matches!(kind, Some(PermissionRequestKind::Question)),
                    ),
                    _ => (Vec::new(), false),
                };
                DelegatedPeek::Found {
                    pending: Arc::clone(pending),
                    options,
                    is_question,
                }
            }
            None => DelegatedPeek::Absent,
        }
    }

    /// The delegated answer, checks one through six **in order**, and the
    /// only door an agent's answer has to a resolution.
    ///
    /// The commission's rule (§4.2): a denial-on-failed-check cannot be
    /// written by accident. The mechanism here is a typestate —
    /// [`CheckedDelegatedCard`] is the only value [`Self::resolve_checked`]
    /// accepts, its fields are private, and its constructor is a private
    /// function of *this module*, called exactly once, at the end of the
    /// check sequence below. A caller that skipped a check has no value to
    /// pass; a failed check returns before the value exists, so it has
    /// nowhere to divert into `respond`. The human path
    /// ([`Self::respond_with_option`]) keeps its own shape and its own trap
    /// semantics; the delegated path never reaches it.
    ///
    /// The checks themselves are supplied as closures because the facts they
    /// read live one layer up: the switch is the daemon's store (read **at
    /// call time**, never cached — the read-cadence rule at
    /// `delegation_store.rs`), the child link and the peer capability are the
    /// registry's rows. Supplying them is testimony; the behaviour tests
    /// (C1/C2/C4/C5) hold that testimony to the real wiring.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn answer_delegated(
        &self,
        tool_call_id: &str,
        outcome: PermissionOutcome,
        switch_on: &dyn Fn() -> bool,
        resolved_elsewhere: &dyn Fn(&str) -> bool,
        child_check: &dyn Fn(&str) -> Result<(), String>,
        caps_check: &dyn Fn(&str) -> Result<(), String>,
        answered_by: &str,
    ) -> Result<(), String> {
        Self::answer_delegated_on(
            Some(self),
            tool_call_id,
            outcome,
            switch_on,
            resolved_elsewhere,
            child_check,
            caps_check,
            answered_by,
        )
    }

    /// The whole delegated-answer chain, checks one through six in order,
    /// against whichever broker holds the card (`Some` when the registry's
    /// scan found it; `None` when no live session's table has it, which is
    /// check 3's territory). The single-broker wrapper above is its natural
    /// test seam.
    ///
    /// The commission's rule (§4.2): a denial-on-failed-check cannot be
    /// written by accident. The mechanism is a typestate —
    /// [`CheckedDelegatedCard`] is the only value [`PermissionBroker::
    /// resolve_checked`] accepts, its fields are private, and its constructor
    /// is a private function of this module with exactly one call site, at
    /// the end of the check sequence below. A caller that skipped a check has
    /// no value to pass; a failed check returns before the value exists, so
    /// it has nowhere to divert into `respond`. The human path
    /// ([`PermissionBroker::respond_with_option`]) keeps its own shape and
    /// its own trap semantics; the delegated path never reaches it.
    ///
    /// The facts some checks read live one layer up: the switch is the
    /// daemon's store (read **at call time**, never cached — the read-cadence
    /// rule at `delegation_store.rs`), the child link and the peer
    /// capability are the registry's rows. Supplying them as closures is
    /// testimony; the behaviour tests (C1/C2/C4/C5) hold that testimony to
    /// the real wiring.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn answer_delegated_on(
        broker: Option<&PermissionBroker>,
        tool_call_id: &str,
        outcome: PermissionOutcome,
        switch_on: &dyn Fn() -> bool,
        resolved_elsewhere: &dyn Fn(&str) -> bool,
        child_check: &dyn Fn(&str) -> Result<(), String>,
        caps_check: &dyn Fn(&str) -> Result<(), String>,
        answered_by: &str,
    ) -> Result<(), String> {
        // Check 0: a first-use write gate is never delegatable, whatever its
        // options look like. The id is the daemon's own namespace, so no
        // caller can mint or shed it; the shape checks below keep working
        // for every other card unchanged.
        if crate::mcp_broker::is_first_use_card(tool_call_id) {
            return Err(format!(
                "permission card {tool_call_id} approves a first-use write gate; only a person can answer it, so it stays pending"
            ));
        }
        // Check 1 (+ the found half of 3): the card's own options, read from
        // the pending entry before any `respond*` call. An outcome the card
        // does not support is a refusal that leaves the card pending — the
        // delegated path never completes a card as cancelled for its own
        // validation failure, which is what the human path's trap does.
        let peek = broker
            .map(|broker| broker.peek_delegated(tool_call_id))
            .unwrap_or(DelegatedPeek::Absent);
        let selected = match &peek {
            DelegatedPeek::Found {
                options,
                is_question,
                ..
            } => {
                // A question belongs to the person, never to a parent
                // agent — even a single-option one no chooser rule marks.
                // The MCP answer door names no option and carries no text,
                // so anything sent here would answer what nobody chose.
                if *is_question {
                    return Err(format!(
                        "permission card {tool_call_id} is a question; only a person answers it, so it stays pending"
                    ));
                }
                // A chooser has no answer this door can give: the MCP tool
                // carries no option id and the envelope the creator saw
                // lists no options, so anything sent here would be the
                // first-pick the wire field exists to remove. Refused with
                // the card still pending — the chooser rule says the
                // question belongs to the person.
                if options_form_a_chooser(options) {
                    return Err(format!(
                        "permission card {tool_call_id} is a chooser; only a person can choose between its options, so it stays pending"
                    ));
                }
                match select_option(options, outcome, None) {
                    Ok(Some(option)) => Some(option),
                    // A durable option is never chosen implicitly, and the
                    // delegated tool names no option at all: a card offering only
                    // `allow_always`/`reject_always` is not answerable here.
                    Ok(None) | Err(_) => {
                        return Err(format!(
                            "this card offers no one-shot option for {outcome:?} (offered: {}); it stays pending for a person",
                            offered_kinds(options)
                        ));
                    }
                }
            }
            DelegatedPeek::Absent => None,
        };
        // Check 2: the switch, read now — the answer that was true when the
        // card was surfaced says nothing about now.
        if !switch_on() {
            return Err(
                "permission delegation is off; the card stays pending for a person".to_string(),
            );
        }
        // Check 3: the card exists. Unknown and already-resolved are two
        // sentences, and both inert.
        let pending = match peek {
            DelegatedPeek::Found { pending, .. } => pending,
            DelegatedPeek::Absent => {
                if resolved_elsewhere(tool_call_id) {
                    return Err(format!(
                        "permission request {tool_call_id} has already been resolved"
                    ));
                }
                return Err(format!("unknown permission card {tool_call_id}"));
            }
        };
        // Check 4: the card's session is a live child of the caller — the
        // registry's `created_by` link, identity taken from the bearer
        // (§0.1), never from the request.
        child_check(&pending.session_id)?;
        // Check 5: a creator whose session belongs to a paired device answers
        // only if that device holds `answer_permissions`.
        caps_check(&pending.session_id)?;
        // Check 6: the single-use take, pinned to the entry the checks read.
        // Resolved in between means the take refuses and nothing happens.
        let (journal_outcome, selected) = match (outcome, selected) {
            (PermissionOutcome::AllowOnce, Some(option)) => ("allow_once", option),
            (PermissionOutcome::Deny, Some(option)) => ("deny", option),
            // Unreachable: `selected` is Some whenever `peek` was Found, and
            // check 3 has already narrowed Found. Spelled rather than
            // `unreachable!()` so the compiler keeps proving the pair.
            (_, None) => {
                return Err(format!("unknown permission card {tool_call_id}"));
            }
        };
        let card = checked_delegated(pending, selected, journal_outcome, answered_by.to_string());
        let broker = broker.expect("a Found peek implies the broker that holds the card");
        broker
            .resolve_checked(card)
            .map_err(|error| error.to_string())
    }

    /// Resolve a card whose checks have all passed. The take is pinned to
    /// the exact entry the checks read, so a card resolved in between is
    /// refused, not re-resolved.
    fn resolve_checked(&self, card: CheckedDelegatedCard) -> Result<(), PermissionResponseError> {
        let pending = self.take(&card.pending.tool_call_id, Some(&card.pending))?;
        let result = serde_json::json!({
            "outcome": { "outcome": "selected", "optionId": card.option.option_id }
        });
        self.complete(
            &pending,
            result,
            Some(&card.option),
            card.journal_outcome,
            Some(&card.answered_by),
        )
    }

    /// Auto-answer the modes `provider_catalog::mode_is_auto_answered` lists —
    /// the one list, shared with the `unattended` marker a child's birth
    /// writes — and only when the agent offers one allow choice; chooser
    /// requests stay with the client, and so does every question: a model's
    /// question is never auto-answered, in any mode. Paseo's chooser rule:
    /// the same kind twice — allow or reject — is a question, the standard
    /// `allow_once`/`allow_always`/`reject_once` batch (three distinct kinds)
    /// is not. Prefer allow_once, then allow_always; a request with no allow
    /// option stays pending for the user. The journal records the kind that
    /// was really granted, never a one-shot constant.
    pub(super) fn auto_answer(
        &self,
        tool_call_id: &str,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<bool, PermissionResponseError> {
        let Some(mode_id) = runtime.current_mode_id() else {
            return Ok(false);
        };
        if !crate::provider_catalog::mode_is_auto_answered(mode_id.as_str()) {
            return Ok(false);
        }
        let Some(AutoAnswer { pending, option }) = self.take_auto_answerable(tool_call_id)? else {
            return Ok(false);
        };
        let result = serde_json::json!({
            "outcome": { "outcome": "selected", "optionId": option.option_id }
        });
        // The daemon answered in the child's own unattended mode: nobody to
        // attribute it to.
        self.complete(&pending, result, Some(&option), &option.kind, None)?;
        Ok(true)
    }

    #[cfg(test)]
    pub(super) fn expire(&self, tool_call_id: &str, expected: &Arc<PendingPermission>) -> bool {
        self.cancel(tool_call_id, expected, "timeout")
    }

    pub(super) fn cancel(
        &self,
        tool_call_id: &str,
        expected: &Arc<PendingPermission>,
        journal_outcome: &str,
    ) -> bool {
        let Ok(pending) = self.take(tool_call_id, Some(expected)) else {
            return false;
        };
        self.complete(
            &pending,
            serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
            None,
            journal_outcome,
            None,
        )
        .is_ok()
    }

    /// Soft interrupt: complete every pending request as cancelled but leave
    /// the broker open, so later turns can register new permissions.
    pub(super) fn cancel_pending(&self) {
        let pending = self
            .pending
            .lock()
            .map(|mut table| table.entries.drain().map(|(_, pending)| pending).collect())
            .unwrap_or_else(|_| Vec::new());
        for card in &pending {
            release_card_slot(&card.request);
        }
        self.complete_cancelled(pending);
    }

    /// Tear the session down: no later request may register.
    pub(super) fn close(&self) {
        let pending = self
            .pending
            .lock()
            .map(|mut table| {
                table.closed = true;
                table.entries.drain().map(|(_, pending)| pending).collect()
            })
            .unwrap_or_else(|_| Vec::new());
        for card in &pending {
            release_card_slot(&card.request);
        }
        self.complete_cancelled(pending);
    }

    fn complete_cancelled(&self, pending: Vec<Arc<PendingPermission>>) {
        for pending in pending {
            let _ = self.complete(
                &pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
                None,
                "cancelled",
                None,
            );
        }
    }

    fn take(
        &self,
        tool_call_id: &str,
        expected: Option<&Arc<PendingPermission>>,
    ) -> Result<Arc<PendingPermission>, PermissionResponseError> {
        let mut table = self
            .pending
            .lock()
            .map_err(|_| io_error("permission broker lock poisoned"))?;
        let Some(current) = table.entries.get(tool_call_id) else {
            return Err(PermissionResponseError::NotFound);
        };
        if let Some(expected) = expected {
            if !Arc::ptr_eq(current, expected) {
                return Err(PermissionResponseError::NotFound);
            }
        }
        let pending = table
            .entries
            .remove(tool_call_id)
            .ok_or(PermissionResponseError::NotFound)?;
        // The card is gone, so the slot it held goes back to its device. This
        // is the one place an entry leaves the table on its own, so an answer,
        // the auto-answer path and a cancel all release through it.
        release_card_slot(&pending.request);
        Ok(pending)
    }

    /// Decide the auto-answer and remove the entry in the same lock. The
    /// entry is removed only once an allow option has been selected, so a
    /// chooser or an allow-less request stays pending for the client.
    fn take_auto_answerable(
        &self,
        tool_call_id: &str,
    ) -> Result<Option<AutoAnswer>, PermissionResponseError> {
        // A first-use gate is never auto-answered, even one-shot: each call
        // must reach the person, and an unattended mode is not the person.
        if crate::mcp_broker::is_first_use_card(tool_call_id) {
            return Ok(None);
        }
        let mut table = self
            .pending
            .lock()
            .map_err(|_| io_error("permission broker lock poisoned"))?;
        let Some(current) = table.entries.get(tool_call_id) else {
            return Err(PermissionResponseError::NotFound);
        };
        let (options, is_question) = match &current.request {
            SessionEvent::PermissionRequest { options, kind, .. } => (
                options.clone(),
                matches!(kind, Some(PermissionRequestKind::Question)),
            ),
            _ => return Ok(None),
        };
        // Semantic, not structural: a question is never auto-answered, in
        // any mode — including bypass or "may run without asking". The
        // chooser rule below stays for every other request.
        if is_question {
            return Ok(None);
        }
        if options_form_a_chooser(&options) {
            return Ok(None);
        }
        let Some(option) = select_allow_option(&options).cloned() else {
            return Ok(None);
        };
        let pending = table
            .entries
            .remove(tool_call_id)
            .ok_or(PermissionResponseError::NotFound)?;
        release_card_slot(&pending.request);
        Ok(Some(AutoAnswer { pending, option }))
    }

    fn complete(
        &self,
        pending: &Arc<PendingPermission>,
        result: serde_json::Value,
        selected_option: Option<&PermissionOption>,
        journal_outcome: &str,
        answered_by: Option<&str>,
    ) -> Result<(), PermissionResponseError> {
        self.note_watched_choice(&pending.tool_call_id, selected_option);
        let runtime = pending.runtime.upgrade();
        let recorded = runtime
            .as_ref()
            .map(|runtime| {
                runtime.record_permission_decision(
                    &pending.tool_call_id,
                    journal_outcome,
                    &pending.request,
                ) || !self.require_journal
            })
            .unwrap_or(!self.require_journal);
        let decision = if recorded {
            decision_from_outcome(journal_outcome)
        } else {
            HostDecision::Cancelled
        };
        if !recorded {
            let send_result = self.dispatch_with_fallback(
                pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
            );
            if let Some(runtime) = runtime {
                runtime.remove_permission_request(&pending.tool_call_id);
                // The card was cancelled, whatever was attempted: the
                // observers must read a cancellation, never the grant.
                let _ = runtime.publish_agent_event(
                    permission_resolved_event(pending, None, "cancelled", answered_by),
                    None,
                );
            }
            self.mark_done(pending, decision);
            return match send_result {
                Ok(()) => Err(PermissionResponseError::Io(io::Error::other(
                    "permission decision was not journaled; ACP request was cancelled",
                ))),
                Err(error) => Err(PermissionResponseError::Io(error)),
            };
        }
        let send_result = self.dispatch_with_fallback(pending, result);
        if let Some(runtime) = runtime {
            runtime.remove_permission_request(&pending.tool_call_id);
            let _ = runtime.publish_agent_event(
                permission_resolved_event(pending, selected_option, journal_outcome, answered_by),
                None,
            );
            // The durable attribution record, on every resolution: the
            // snapshot's delegation count is read back from what the journal
            // survived, and the app's replayed ledger has no other source.
            let _ = runtime.publish_agent_event(
                permission_answered_event(pending, answered_by, journal_outcome),
                Some(
                    &serde_json::to_string(&permission_answered_event(
                        pending,
                        answered_by,
                        journal_outcome,
                    ))
                    .unwrap_or_default(),
                ),
            );
        }
        self.mark_done(pending, decision);
        send_result.map_err(PermissionResponseError::Io)
    }

    fn dispatch_responder(
        &self,
        pending: &PendingPermission,
        result: serde_json::Value,
    ) -> io::Result<()> {
        match pending.responder {
            PermissionResponder::Agent { acp_id } => (self.sender)(acp_id, result),
            PermissionResponder::Host => Ok(()),
        }
    }

    fn dispatch_with_fallback(
        &self,
        pending: &PendingPermission,
        result: serde_json::Value,
    ) -> io::Result<()> {
        match self.dispatch_responder(pending, result) {
            Ok(()) => Ok(()),
            Err(error) => match self.dispatch_responder(
                pending,
                serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
            ) {
                Ok(()) => Ok(()),
                Err(_) => Err(error),
            },
        }
    }

    fn mark_done(&self, pending: &Arc<PendingPermission>, decision: HostDecision) {
        let (done, wake) = &*pending.done;
        if let Ok(mut completed) = done.lock() {
            if completed.done {
                return;
            }
            completed.done = true;
            completed.decision = Some(decision);
            wake.notify_all();
        }
    }

    /// Test-only road into the pending table for suites outside the session
    /// module: the write gate's own tests answer the card a human answers.
    #[cfg(test)]
    pub(crate) fn test_pending_ids(&self) -> Vec<String> {
        self.pending_ids()
    }

    /// Read one pending card's request, without touching it — what the
    /// card shows a person, for the tests that pin a card's facts.
    #[cfg(test)]
    pub(crate) fn test_pending_request(&self, tool_call_id: &str) -> Option<SessionEvent> {
        self.pending
            .lock()
            .ok()?
            .entries
            .get(tool_call_id)
            .map(|pending| pending.request.clone())
    }

    /// Answer one pending card with the named option, as a human does.
    #[cfg(test)]
    pub(crate) fn test_answer(
        &self,
        tool_call_id: &str,
        outcome: PermissionOutcome,
        option_id: &str,
    ) -> Result<(), String> {
        self.respond_with_option(tool_call_id, outcome, Some(option_id.to_string()), None)
            .map_err(|error| error.to_string())
    }

    /// Watch one card's answer for a first-use gate: the next `complete`
    /// of this id remembers which option was chosen.
    pub(crate) fn watch_card_choice(&self, tool_call_id: &str) {
        if let Ok(mut watch) = self.watch_choices.lock() {
            watch.insert(tool_call_id.to_string());
        }
    }

    /// Take a watched card's answer: the option id it was answered with,
    /// or `None` when it was answered without an option. `None` itself
    /// means the card never completed. Un-watches either way.
    pub(crate) fn take_card_choice(&self, tool_call_id: &str) -> Option<Option<String>> {
        if let Ok(mut watch) = self.watch_choices.lock() {
            watch.remove(tool_call_id);
        }
        self.choice_answers.lock().ok()?.remove(tool_call_id)
    }

    fn note_watched_choice(&self, tool_call_id: &str, selected_option: Option<&PermissionOption>) {
        if let Ok(mut watch) = self.watch_choices.lock() {
            if watch.remove(tool_call_id) {
                if let Ok(mut answers) = self.choice_answers.lock() {
                    answers.insert(
                        tool_call_id.to_string(),
                        selected_option.map(|option| option.option_id.clone()),
                    );
                }
            }
        }
    }

    pub(super) fn pending_len(&self) -> usize {
        self.pending
            .lock()
            .map(|table| table.entries.len())
            .unwrap_or(0)
    }

    /// At most `limit` of this session's pending cards, as `(cardId, request)`
    /// clones in card-id order: keys are sorted first and only the first
    /// `limit` are cloned, so a caller that will keep a bounded number of
    /// cards never pays — under this lock — for the ones it would drop.
    /// Nothing is taken: the cards stay pending for whoever answers them.
    pub(super) fn pending_cards(&self, limit: usize) -> Vec<(String, SessionEvent)> {
        let Ok(table) = self.pending.lock() else {
            return Vec::new();
        };
        let mut ids: Vec<&String> = table.entries.keys().collect();
        ids.sort();
        ids.truncate(limit);
        ids.into_iter()
            .map(|id| (id.clone(), table.entries[id].request.clone()))
            .collect()
    }

    /// Register a host-initiated permission, publish it, and block until the
    /// user or cancellation decides. The ACP agent is not written to.
    pub(super) fn request_host_permission(
        self: &Arc<Self>,
        request: SessionEvent,
        runtime: &Arc<SessionRuntime>,
    ) -> HostDecision {
        let pending = match self.register_host(request, runtime) {
            Ok(pending) => pending,
            Err(_) => return HostDecision::Cancelled,
        };
        if runtime.permission_delivery_enabled() == Some(false) {
            let _ = self.cancel(&pending.tool_call_id, &pending, "capability_not_supported");
            return HostDecision::Cancelled;
        }
        // The card the client is shown is the registered one, origin included:
        // the caller's own event never carries a stamp.
        let _ = runtime.publish_agent_event(pending.request.clone(), None);
        self.wait_for_decision(&pending)
    }

    fn wait_for_decision(&self, pending: &PendingPermission) -> HostDecision {
        let (done, wake) = &*pending.done;
        let Ok(mut completed) = done.lock() else {
            return HostDecision::Cancelled;
        };
        while !completed.done {
            let Ok(next) = wake.wait(completed) else {
                return HostDecision::Cancelled;
            };
            completed = next;
        }
        completed.decision.unwrap_or(HostDecision::Cancelled)
    }

    #[cfg(test)]
    pub(super) fn pending_ids(&self) -> Vec<String> {
        self.pending
            .lock()
            .map(|table| table.entries.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[cfg(test)]
    fn set_after_take_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut stored) = self.after_take_hook.lock() {
            *stored = Some(hook);
        }
    }

    #[cfg(test)]
    fn run_after_take_hook(&self) {
        let hook = self
            .after_take_hook
            .lock()
            .ok()
            .and_then(|mut stored| stored.take());
        if let Some(hook) = hook {
            hook();
        }
    }
}

/// The pending permission is no longer waiting, as the observers see it.
///
/// The decision kind is not the secret: a text answer names no option, but
/// the grant it carries is still `allow_once`, and observers must read an
/// allowed question as allowed — never as unclaimed. Only a grant falls
/// back this way; a cancellation names nothing either way.
fn permission_resolved_event(
    pending: &PendingPermission,
    selected_option: Option<&PermissionOption>,
    journal_outcome: &str,
    answered_by: Option<&str>,
) -> SessionEvent {
    let granted_kind = match journal_outcome {
        "allow_once" | "allow_always" => Some(journal_outcome.to_string()),
        _ => None,
    };
    SessionEvent::PermissionResolved {
        tool_call_id: pending.tool_call_id.clone(),
        selected_option_id: selected_option.map(|option| option.option_id.clone()),
        selected_option_kind: selected_option
            .map(|option| option.kind.clone())
            .or(granted_kind),
        selected_option_name: selected_option.map(|option| option.name.clone()),
        answered_by: answered_by.map(str::to_string),
    }
}

/// The durable attribution record for one resolution. Every resolution
/// carries it — a cancellation and an auto-answer answer `None` exactly as a
/// person's answer does — so the replayed count and the live ledger count the
/// same events.
fn permission_answered_event(
    pending: &PendingPermission,
    answered_by: Option<&str>,
    journal_outcome: &str,
) -> SessionEvent {
    SessionEvent::PermissionAnswered {
        card_id: pending.tool_call_id.clone(),
        answered_by: answered_by.map(str::to_string),
        outcome: journal_outcome.to_string(),
    }
}

/// The same event with the session's origin written into it, when it is a
/// permission request.
///
/// Every field is named, deliberately: a pattern with `..` would silently drop
/// the next field this variant gains, and the card's provenance line is
/// written by whoever adds it. A compile error here is the reminder.
///
/// `origin` is not `Option`, so this **overwrites** whatever a provider client
/// put there: the placeholder cannot survive to the wire, and a peer session's
/// card cannot be mislabelled as this machine's own. Called from the broker
/// (the pending entry carries the truth for the per-peer card count) and from
/// `SessionRuntime::publish_agent_event_with_seq`, which is the one place a
/// request leaves for a subscriber.
pub(super) fn stamp_origin(request: SessionEvent, origin: SessionOrigin) -> SessionEvent {
    match request {
        SessionEvent::PermissionRequest {
            tool_call_id,
            title,
            description,
            command,
            args,
            cwd,
            env,
            options,
            origin: _,
            create_agent,
            is_chooser,
            kind,
            questions,
        } => SessionEvent::PermissionRequest {
            tool_call_id,
            title,
            description,
            command,
            args,
            cwd,
            env,
            options,
            is_chooser,
            kind,
            questions,
            origin,
            create_agent,
        },
        other => other,
    }
}

/// The same event with the daemon's chooser verdict written into it, when it
/// is a permission request: `Some(true)` exactly when the option set trips the
/// rule [`options_form_a_chooser`] names, absent otherwise.
///
/// Called from `PermissionBroker::register_with`, where the copy the audit row
/// and the replay roads read becomes pending, and from
/// `SessionRuntime::publish_agent_event_with_seq` on the way out — the one
/// place a request leaves for a subscriber — so every copy that can reach the
/// app carries the verdict, and the card renders one control per option
/// without ever re-deriving the rule from the option list. Like the origin
/// stamp this **overwrites**: a value a provider client wrote cannot survive
/// to the wire.
pub(super) fn stamp_chooser(mut request: SessionEvent) -> SessionEvent {
    if let SessionEvent::PermissionRequest {
        options,
        is_chooser,
        ..
    } = &mut request
    {
        // A card that already says it is a chooser keeps saying so: some
        // cards need named choices without repeating a kind, and the stamp
        // must not downgrade them to the generic pair.
        *is_chooser = (*is_chooser).or(options_form_a_chooser(options).then_some(true));
    }
    request
}

/// The device id of a request's origin, when it came from a paired device.
fn peer_origin_device(request: &SessionEvent) -> Option<String> {
    match request {
        SessionEvent::PermissionRequest { origin, .. } => (origin.kind == SessionOriginKind::Peer)
            .then(|| origin.device_id.clone())
            .flatten(),
        _ => None,
    }
}

fn validate_permission_request(
    tool_call_id: &str,
    request: &SessionEvent,
) -> Result<(), PermissionResponseError> {
    validate_permission_field("tool_call_id", tool_call_id)?;
    let SessionEvent::PermissionRequest {
        title,
        description,
        command,
        args,
        cwd,
        env,
        options,
        questions,
        kind,
        ..
    } = request
    else {
        return Err(PermissionResponseError::InvalidRequest(
            "not a permission request".to_string(),
        ));
    };
    validate_permission_field("title", title)?;
    for (field, value) in [
        ("description", description.as_deref()),
        ("command", command.as_deref()),
        ("cwd", cwd.as_deref()),
    ] {
        if let Some(value) = value {
            validate_permission_field(field, value)?;
        }
    }
    if let Some(args) = args {
        if args.len() > MAX_ACP_PERMISSION_ARGS {
            return Err(PermissionResponseError::InvalidRequest(format!(
                "permission request has more than the maximum of {MAX_ACP_PERMISSION_ARGS} args"
            )));
        }
        for arg in args {
            if arg.len() > MAX_ACP_PERMISSION_FIELD_BYTES {
                return Err(PermissionResponseError::InvalidRequest(format!(
                    "permission request arg exceeds {MAX_ACP_PERMISSION_FIELD_BYTES} bytes"
                )));
            }
        }
    }
    if let Some(env) = env {
        if env.len() > MAX_ACP_PERMISSION_ENV {
            return Err(PermissionResponseError::InvalidRequest(format!(
                "permission request has more than the maximum of {MAX_ACP_PERMISSION_ENV} env vars"
            )));
        }
        for variable in env {
            validate_permission_field("env name", &variable.name)?;
            if variable.value.len() > MAX_ACP_PERMISSION_FIELD_BYTES {
                return Err(PermissionResponseError::InvalidRequest(format!(
                    "permission request env value exceeds {MAX_ACP_PERMISSION_FIELD_BYTES} bytes"
                )));
            }
        }
    }
    // A free-text-only question offers no options: the person answers in
    // the "Other" field, so emptiness is legal exactly then.
    if options.is_empty() && !matches!(kind, Some(PermissionRequestKind::Question)) {
        return Err(PermissionResponseError::InvalidRequest(
            "permission request has no options".to_string(),
        ));
    }
    if options.len() > MAX_ACP_PERMISSION_OPTIONS {
        return Err(PermissionResponseError::InvalidRequest(format!(
            "permission request has more than the maximum of {MAX_ACP_PERMISSION_OPTIONS} options"
        )));
    }
    let mut option_ids: Vec<&str> = Vec::new();
    for option in options {
        validate_permission_field("option_id", &option.option_id)?;
        validate_permission_field("option name", &option.name)?;
        validate_permission_field("option kind", &option.kind)?;
        // Two options cannot share the id the answer names: resolution is
        // the first match, so a shared id would tell the agent one thing
        // about "Beta" and the card another (review A2a #10).
        if option_ids.contains(&option.option_id.as_str()) {
            return Err(PermissionResponseError::InvalidRequest(format!(
                "permission request has two options with the id '{}'",
                option.option_id
            )));
        }
        option_ids.push(&option.option_id);
    }
    if let Some(questions) = questions {
        // A question's text is the agent's own words — bounded like every
        // other card field, but never confused with the person's answer,
        // which has no field on the request at all.
        if questions.len() > MAX_QUESTIONS_PER_REQUEST {
            return Err(PermissionResponseError::InvalidRequest(format!(
                "permission request has more than the maximum of {MAX_QUESTIONS_PER_REQUEST} questions"
            )));
        }
        for question in questions {
            validate_permission_field("question", &question.question)?;
            if let Some(header) = question.header.as_deref() {
                validate_permission_field("question header", header)?;
            }
            if question.options.len() > MAX_ACP_PERMISSION_OPTIONS {
                return Err(PermissionResponseError::InvalidRequest(format!(
                    "permission question has more than the maximum of {MAX_ACP_PERMISSION_OPTIONS} options"
                )));
            }
            for option in &question.options {
                validate_permission_field("question option label", &option.label)?;
                if let Some(description) = option.description.as_deref() {
                    validate_permission_field("question option description", description)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_permission_field(field: &str, value: &str) -> Result<(), PermissionResponseError> {
    if value.is_empty() {
        return Err(PermissionResponseError::InvalidRequest(format!(
            "permission request has an empty {field}"
        )));
    }
    if value.len() > MAX_ACP_PERMISSION_FIELD_BYTES {
        return Err(PermissionResponseError::InvalidRequest(format!(
            "permission request {field} exceeds {MAX_ACP_PERMISSION_FIELD_BYTES} bytes"
        )));
    }
    Ok(())
}

fn decision_from_outcome(journal_outcome: &str) -> HostDecision {
    // Invariant: only the allow kinds grant a spawn. Any new journal outcome
    // that is not mapped here is a deny (Cancelled) — the catch-all is
    // deliberate, not a leftover default.
    match journal_outcome {
        "allow_once" | "allow_always" | "allow_session" => HostDecision::Allow,
        "deny" => HostDecision::Deny,
        "timeout" => HostDecision::Timeout,
        _ => HostDecision::Cancelled,
    }
}

fn select_option(
    options: &[PermissionOption],
    outcome: PermissionOutcome,
    option_id: Option<&str>,
) -> Result<Option<PermissionOption>, String> {
    if let Some(option_id) = option_id {
        let Some(option) = options.iter().find(|option| option.option_id == option_id) else {
            return Err(format!("Unknown permission option '{option_id}'."));
        };
        let valid = match outcome {
            PermissionOutcome::AllowOnce => option.kind.starts_with("allow"),
            PermissionOutcome::Deny => option.kind.starts_with("reject"),
        };
        if !valid {
            return Err(format!(
                "Permission option '{option_id}' cannot be used for {outcome:?}."
            ));
        }
        return Ok(Some(option.clone()));
    }
    let (once_kind, intent) = match outcome {
        PermissionOutcome::AllowOnce => ("allow_once", "allow"),
        PermissionOutcome::Deny => ("reject_once", "reject"),
    };
    if let Some(option) = options.iter().find(|option| option.kind == once_kind) {
        return Ok(Some(option.clone()));
    }
    // A durable option of the same intent is never answered implicitly: the
    // client has to name it, and the request stays pending until then.
    if options.iter().any(|option| option.kind.starts_with(intent)) {
        return Err(format!(
            "Permission request offers no '{once_kind}' option (offered: {}); the request stays pending",
            offered_kinds(options)
        ));
    }
    Ok(None)
}

/// Paseo's chooser rule, on either side: the same kind offered twice —
/// allow or reject — means the agent is asking which one to use, so the
/// request must reach the user.
fn options_form_a_chooser(options: &[PermissionOption]) -> bool {
    let mut seen: Vec<&str> = Vec::new();
    for option in options {
        if seen.contains(&option.kind.as_str()) {
            return true;
        }
        seen.push(&option.kind);
    }
    false
}

/// The auto-accept order Paseo uses: one-shot first, then durable.
fn select_allow_option(options: &[PermissionOption]) -> Option<&PermissionOption> {
    options
        .iter()
        .find(|option| option.kind == "allow_once")
        .or_else(|| options.iter().find(|option| option.kind == "allow_always"))
}

fn offered_kinds(options: &[PermissionOption]) -> String {
    options
        .iter()
        .map(|option| option.kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn unsupported_outcome_reason(options: &[PermissionOption], outcome: PermissionOutcome) -> String {
    let label = match outcome {
        PermissionOutcome::AllowOnce => "Allow once",
        PermissionOutcome::Deny => "Deny",
    };
    let intent = match outcome {
        PermissionOutcome::AllowOnce => "allow",
        PermissionOutcome::Deny => "reject",
    };
    format!(
        "Could not honor {label}: ACP did not offer any {intent} option (offered: {}); request was cancelled",
        offered_kinds(options)
    )
}

fn io_error(message: &str) -> PermissionResponseError {
    PermissionResponseError::Io(io::Error::other(message))
}

#[cfg(test)]
use std::path::PathBuf;
#[cfg(test)]
#[cfg(test)]
pub(super) fn permission_with_kinds(tool_call_id: &str, kinds: &[(&str, &str)]) -> SessionEvent {
    SessionEvent::PermissionRequest {
        tool_call_id: tool_call_id.to_string(),
        title: "Run command".to_string(),
        description: None,
        command: Some("echo test".to_string()),
        args: None,
        cwd: None,
        env: None,
        options: kinds
            .iter()
            .map(|(option_id, kind)| PermissionOption {
                option_id: (*option_id).to_string(),
                name: (*kind).to_string(),
                kind: (*kind).to_string(),
            })
            .collect(),
        // A provider client writes `local` as a placeholder; the daemon
        // overwrites it with the session's own origin before the request leaves
        // for a subscriber.
        origin: SessionOrigin::local(),
        create_agent: None,
        // The publish path stamps the chooser verdict on the way out, the
        // same way it stamps the origin.
        is_chooser: None,
        kind: None,
        questions: None,
    }
}

#[cfg(test)]
pub(super) fn permission_question_single_option(tool_call_id: &str) -> SessionEvent {
    use devboule_protocol::{PermissionQuestion, PermissionQuestionOption, PermissionRequestKind};
    // One offered label: structurally auto-answerable (no repeated kind),
    // so only the semantic question gate keeps it pending. This is the
    // case the chooser heuristic cannot see.
    SessionEvent::PermissionRequest {
        tool_call_id: tool_call_id.to_string(),
        title: "Shall I paint the fence green?".to_string(),
        description: None,
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![PermissionOption {
            option_id: "q0o0".to_string(),
            name: "Yes, green".to_string(),
            kind: "allow_once".to_string(),
        }],
        origin: SessionOrigin::local(),
        create_agent: None,
        is_chooser: None,
        kind: Some(PermissionRequestKind::Question),
        questions: Some(vec![PermissionQuestion {
            question: "Shall I paint the fence green?".to_string(),
            header: None,
            options: vec![PermissionQuestionOption {
                label: "Yes, green".to_string(),
                description: None,
            }],
            multi_select: false,
        }]),
    }
}

#[cfg(test)]
pub(super) fn permission_question(tool_call_id: &str) -> SessionEvent {
    use devboule_protocol::{PermissionQuestion, PermissionQuestionOption, PermissionRequestKind};
    SessionEvent::PermissionRequest {
        tool_call_id: tool_call_id.to_string(),
        title: "Which colour should I paint the fence?".to_string(),
        description: Some("Forest green (Recommended) / Barn red".to_string()),
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            PermissionOption {
                option_id: "q0o0".to_string(),
                name: "Forest green (Recommended)".to_string(),
                kind: "allow_once".to_string(),
            },
            PermissionOption {
                option_id: "q0o1".to_string(),
                name: "Barn red".to_string(),
                kind: "allow_once".to_string(),
            },
        ],
        // A provider client writes `local` as a placeholder; the daemon
        // overwrites it with the session's own origin before the request leaves
        // for a subscriber.
        origin: SessionOrigin::local(),
        create_agent: None,
        // The publish path stamps the chooser verdict on the way out, the
        // same way it stamps the origin.
        is_chooser: None,
        kind: Some(PermissionRequestKind::Question),
        questions: Some(vec![PermissionQuestion {
            question: "Which colour should I paint the fence?".to_string(),
            header: None,
            options: vec![
                PermissionQuestionOption {
                    label: "Forest green (Recommended)".to_string(),
                    description: None,
                },
                PermissionQuestionOption {
                    label: "Barn red".to_string(),
                    description: None,
                },
            ],
            multi_select: false,
        }]),
    }
}

#[cfg(test)]
pub(super) fn permission(tool_call_id: &str) -> SessionEvent {
    permission_with_kinds(
        tool_call_id,
        &[("allow", "allow_once"), ("deny", "reject_once")],
    )
}

#[cfg(test)]
pub(super) fn permission_path(label: &str) -> PathBuf {
    crate::test_dirs::test_temp_dir(&format!("devboule-permission-{label}")).join("broker.sqlite")
}

#[cfg(test)]
pub(super) fn test_broker() -> (Arc<PermissionBroker>, Arc<Mutex<SentResponses>>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let sent_for_sender = Arc::clone(&sent);
    let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
        sent_for_sender
            .lock()
            .expect("sent lock")
            .push((id, result));
        Ok(())
    });
    (PermissionBroker::for_test(sender), sent)
}

#[cfg(test)]
pub(super) type SentResponses = Vec<(u64, serde_json::Value)>;

#[cfg(test)]
mod tests {
    use super::SessionRuntime;
    use super::{
        peer_card_count, permission, permission_path, permission_with_kinds, test_broker,
        PermissionBroker, PermissionSender, MAX_ACP_PERMISSION_ARGS, MAX_PENDING_ACP_PERMISSIONS,
        MAX_PENDING_FOR_PEER,
    };
    use crate::journal::Journal;
    use devboule_protocol::{PeerRole, PermissionOutcome, SessionEvent, SessionOrigin};
    use rusqlite::Connection;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    #[test]
    fn legacy_allow_without_a_one_shot_option_stays_pending() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                51,
                permission_with_kinds("durable-only", &[("always", "allow_always")]),
                &runtime,
            )
            .expect("register");

        let error = broker
            .respond("durable-only", PermissionOutcome::AllowOnce)
            .expect_err("a durable allow is never chosen implicitly");
        assert!(error.to_string().contains("allow_once"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());

        broker
            .respond_with_option(
                "durable-only",
                PermissionOutcome::AllowOnce,
                Some("always".to_string()),
                None,
            )
            .expect("explicit durable option");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1["outcome"]["optionId"], "always");
        assert_eq!(broker.pending_len(), 0);
    }

    #[test]
    fn explicit_option_id_is_honoured_and_reported() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                59,
                permission_with_kinds(
                    "explicit",
                    &[("allow-once", "allow_once"), ("always", "allow_always")],
                ),
                &runtime,
            )
            .expect("register");

        broker
            .respond_with_option(
                "explicit",
                PermissionOutcome::AllowOnce,
                Some("allow-once".to_string()),
                None,
            )
            .expect("explicit option");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent[0].1["outcome"]["optionId"], "allow-once");
    }

    #[test]
    fn explicit_option_with_wrong_intent_stays_pending() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                60,
                permission_with_kinds("wrong-intent", &[("deny", "reject_once")]),
                &runtime,
            )
            .expect("register");

        let error = broker
            .respond_with_option(
                "wrong-intent",
                PermissionOutcome::AllowOnce,
                Some("deny".to_string()),
                None,
            )
            .expect_err("wrong intent");
        assert!(error.to_string().contains("cannot be used"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    #[test]
    fn legacy_response_without_an_intent_cancels_with_a_reason() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                62,
                permission_with_kinds("no-allow", &[("deny", "reject_once")]),
                &runtime,
            )
            .expect("register");

        let error = broker
            .respond("no-allow", PermissionOutcome::AllowOnce)
            .expect_err("missing intent");
        assert!(error.to_string().contains("did not offer any allow option"));
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(broker.pending_len(), 0);
    }

    #[test]
    fn bypass_mode_auto_answers_without_a_client_permission_request() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(53, permission("pi-bypass"), &runtime)
            .expect("register");

        assert!(broker.auto_answer("pi-bypass", &runtime).expect("answer"));
        assert_eq!(broker.pending_len(), 0);
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent[0].0, 53);
        assert_eq!(sent[0].1["outcome"]["optionId"], "allow");
    }

    #[test]
    fn bypass_mode_leaves_a_duplicate_allow_chooser_for_the_client() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                56,
                permission_with_kinds(
                    "pi-chooser",
                    &[("once", "allow_once"), ("once-again", "allow_once")],
                ),
                &runtime,
            )
            .expect("register");

        assert!(!broker
            .auto_answer("pi-chooser", &runtime)
            .expect("chooser policy"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    /// Review A2a #6 — the rule is not allow-side: a repeated reject kind
    /// is the agent asking which refusal to take, and that question stays
    /// the person's just like a repeated allow kind.
    #[test]
    fn bypass_mode_leaves_a_reject_side_chooser_for_the_client() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                64,
                permission_with_kinds(
                    "reject-chooser",
                    &[
                        ("deploy", "allow_once"),
                        ("skip", "reject_once"),
                        ("cancel", "reject_once"),
                    ],
                ),
                &runtime,
            )
            .expect("register");

        assert!(!broker
            .auto_answer("reject-chooser", &runtime)
            .expect("chooser policy"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    #[test]
    fn bypass_mode_auto_answers_the_standard_option_triple() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                63,
                permission_with_kinds(
                    "pi-standard",
                    &[
                        ("once", "allow_once"),
                        ("always", "allow_always"),
                        ("deny", "reject_once"),
                    ],
                ),
                &runtime,
            )
            .expect("register");

        assert!(broker.auto_answer("pi-standard", &runtime).expect("answer"));
        assert_eq!(broker.pending_len(), 0);
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["optionId"],
            "once"
        );
    }

    #[test]
    fn bypass_mode_leaves_a_request_without_an_allow_option_for_the_client() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                64,
                permission_with_kinds("pi-reject-only", &[("deny", "reject_once")]),
                &runtime,
            )
            .expect("register");

        assert!(!broker
            .auto_answer("pi-reject-only", &runtime)
            .expect("reject-only policy"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    #[test]
    fn every_allow_kind_is_an_allow_decision() {
        use super::{decision_from_outcome, HostDecision};
        assert_eq!(decision_from_outcome("allow_once"), HostDecision::Allow);
        assert_eq!(decision_from_outcome("allow_always"), HostDecision::Allow);
        assert_eq!(decision_from_outcome("allow_session"), HostDecision::Allow);
        assert_eq!(decision_from_outcome("deny"), HostDecision::Deny);
        assert_eq!(decision_from_outcome("timeout"), HostDecision::Timeout);
        assert_eq!(decision_from_outcome("cancelled"), HostDecision::Cancelled);
    }

    /// The first-use gate's session word journals as itself: a session
    /// licence and a one-shot must be distinguishable in history.
    #[test]
    fn a_session_choice_the_person_named_is_journaled_as_session() {
        let path = permission_path("session-human");
        let journal = Arc::new(Journal::open(&path).expect("journal"));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sent_for_sender = Arc::clone(&sent);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            sent_for_sender
                .lock()
                .expect("sent lock")
                .push((id, result));
            Ok(())
        });
        let broker = PermissionBroker::with_sender(sender);
        let runtime = SessionRuntime::for_acp(
            "s.permission.session-human".to_string(),
            Some(Arc::clone(&journal)),
            Arc::clone(&broker),
        );
        broker
            .register(
                68,
                permission_with_kinds(
                    "session-human",
                    &[("once", "allow_once"), ("session", "allow_session")],
                ),
                &runtime,
            )
            .expect("register");

        broker
            .respond_with_option(
                "session-human",
                PermissionOutcome::AllowOnce,
                Some("session".to_string()),
                None,
            )
            .expect("the named session option is honored");
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["optionId"],
            "session"
        );
        journal.flush().expect("journal flush");
        let conn = Connection::open(&path).expect("inspect journal");
        let outcome: String = conn
            .query_row(
                "SELECT outcome FROM permissions WHERE session_id = ?1 AND request_id = ?2",
                ["s.permission.session-human", "session-human"],
                |row| row.get(0),
            )
            .expect("permission row");
        assert_eq!(outcome, "allow_session");
        drop(conn);
        journal.shutdown();
        let _ = std::fs::remove_file(path);
    }

    /// A first-use gate is never auto-answered, not even one-shot: each
    /// call must reach the person, and an unattended mode is not the person.
    #[test]
    fn auto_answer_refuses_first_use_cards() {
        let path = permission_path("session-auto");
        let journal = Arc::new(Journal::open(&path).expect("journal"));
        let (broker, sent) = test_broker();
        let runtime = SessionRuntime::for_acp(
            "s.permission.session-auto".to_string(),
            Some(Arc::clone(&journal)),
            Arc::clone(&broker),
        );
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("grok".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "auto_accept".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                69,
                permission_with_kinds(
                    "write:workspaces:s:1-1",
                    &[("once", "allow_once"), ("session", "allow_session")],
                ),
                &runtime,
            )
            .expect("register");

        assert!(!broker
            .auto_answer("write:workspaces:s:1-1", &runtime)
            .expect("a gate card is not answerable"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
        journal.shutdown();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn bypass_mode_journals_the_durable_allow_it_granted() {
        let path = permission_path("auto-answer");
        let journal = Arc::new(Journal::open(&path).expect("journal"));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sent_for_sender = Arc::clone(&sent);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            sent_for_sender
                .lock()
                .expect("sent lock")
                .push((id, result));
            Ok(())
        });
        let broker = PermissionBroker::with_sender(sender);
        let runtime = SessionRuntime::for_acp(
            "s.permission.auto".to_string(),
            Some(Arc::clone(&journal)),
            Arc::clone(&broker),
        );
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("grok".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "auto_accept".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                65,
                permission_with_kinds("durable-auto", &[("always", "allow_always")]),
                &runtime,
            )
            .expect("register");

        assert!(broker
            .auto_answer("durable-auto", &runtime)
            .expect("answer"));
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["optionId"],
            "always"
        );
        journal.flush().expect("journal flush");
        let conn = Connection::open(&path).expect("inspect journal");
        let outcome: String = conn
            .query_row(
                "SELECT outcome FROM permissions WHERE session_id = ?1 AND request_id = ?2",
                ["s.permission.auto", "durable-auto"],
                |row| row.get(0),
            )
            .expect("permission row");
        assert_eq!(outcome, "allow_always");
        drop(conn);
        journal.shutdown();
        let _ = std::fs::remove_file(path);
    }

    /// The same rule held to the person's own door (review A2a #2): a
    /// durable option the person names is durable in the ledger too, and
    /// the card's resolution reads the granted kind — never a one-shot
    /// constant for a grant that is not one-shot.
    #[test]
    fn a_durable_choice_the_person_named_is_journaled_as_durable() {
        let path = permission_path("durable-human");
        let journal = Arc::new(Journal::open(&path).expect("journal"));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sent_for_sender = Arc::clone(&sent);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            sent_for_sender
                .lock()
                .expect("sent lock")
                .push((id, result));
            Ok(())
        });
        let broker = PermissionBroker::with_sender(sender);
        let runtime = SessionRuntime::for_acp(
            "s.permission.durable-human".to_string(),
            Some(Arc::clone(&journal)),
            Arc::clone(&broker),
        );
        broker
            .register(
                66,
                permission_with_kinds(
                    "durable-human",
                    &[("once", "allow_once"), ("always", "allow_always")],
                ),
                &runtime,
            )
            .expect("register");

        broker
            .respond_with_option(
                "durable-human",
                PermissionOutcome::AllowOnce,
                Some("always".to_string()),
                None,
            )
            .expect("the named durable option is honored");
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["optionId"],
            "always"
        );
        journal.flush().expect("journal flush");
        let conn = Connection::open(&path).expect("inspect journal");
        let outcome: String = conn
            .query_row(
                "SELECT outcome FROM permissions WHERE session_id = ?1 AND request_id = ?2",
                ["s.permission.durable-human", "durable-human"],
                |row| row.get(0),
            )
            .expect("permission row");
        assert_eq!(outcome, "allow_always");
        drop(conn);
        journal.shutdown();
        let _ = std::fs::remove_file(path);
    }

    /// Review A2a #4 — the registered copy is the durable one: the audit
    /// row and the replay road read what `register_with` stored, so the
    /// verdict has to be stamped there too, not only on the way out.
    #[test]
    fn a_journalled_chooser_replays_with_its_mark() {
        let (broker, _sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        let pending = broker
            .register(
                67,
                permission_with_kinds(
                    "chooser-journal",
                    &[("a", "allow_once"), ("b", "allow_once")],
                ),
                &runtime,
            )
            .expect("register");
        let record = crate::journal::agent_report_record(
            "s.chooser.journal".to_string(),
            1,
            1,
            &pending.request,
        )
        .expect("the row serialises");
        let path = permission_path("chooser-journal");
        let journal = Journal::open(&path).expect("journal");
        journal
            .upsert_blocking(crate::journal::new_session_record(
                "s.chooser.journal",
                "tester",
                None,
                devboule_protocol::SessionKind::Terminal,
                "chooser",
            ))
            .expect("session row");
        journal.append_blocking(record).expect("row lands");
        let replay = journal.replay("s.chooser.journal").expect("replay");
        let marked = replay.events.iter().find_map(|event| match event {
            SessionEvent::PermissionRequest { is_chooser, .. } => Some(*is_chooser),
            _ => None,
        });
        assert_eq!(
            marked,
            Some(Some(true)),
            "a chooser replayed from the journal comes back marked"
        );
        journal.shutdown();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_deny_without_a_one_shot_option_stays_pending() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                57,
                permission_with_kinds("reject-always", &[("always", "reject_always")]),
                &runtime,
            )
            .expect("register");

        let error = broker
            .respond("reject-always", PermissionOutcome::Deny)
            .expect_err("a durable reject is never chosen implicitly");
        assert!(error.to_string().contains("reject_once"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    /// Review A2a #3 — a deny ACP cannot carry as a rejection is delivered
    /// as a cancellation, which IS ACP's refusal for this request: an
    /// answer, not an error the person is shown and not a card put back in
    /// the queue.
    #[test]
    fn a_deny_with_no_reject_option_is_delivered_as_a_cancellation_not_an_error() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                68,
                permission_with_kinds("no-reject", &[("a", "allow_once"), ("b", "allow_once")]),
                &runtime,
            )
            .expect("register");

        broker
            .respond("no-reject", PermissionOutcome::Deny)
            .expect("one cancellation was delivered, so one refusal was given");
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
            "cancelled",
            "the agent gets what ACP lets it get"
        );
        assert_eq!(
            broker.pending_len(),
            0,
            "the card is resolved — neither pending nor an error"
        );
    }

    /// Review A2a #10 — two options cannot share the id the answer names:
    /// the first match would win, and the card and the daemon would then
    /// disagree about which one was pressed.
    #[test]
    fn a_request_repeating_an_option_id_is_refused_at_registration() {
        let (broker, _sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        let error = match broker.register(
            69,
            permission_with_kinds("dup-id", &[("x", "allow_once"), ("x", "allow_always")]),
            &runtime,
        ) {
            Ok(_) => panic!("two options cannot share an id the answer names"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "permission request has two options with the id 'x'"
        );
        assert_eq!(broker.pending_len(), 0, "a refused request parks nothing");
    }

    #[test]
    fn auto_answer_falls_back_to_a_definite_cancel_after_send_failure() {
        let first_attempt = Arc::new(AtomicBool::new(true));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let first_attempt_for_sender = Arc::clone(&first_attempt);
        let sent_for_sender = Arc::clone(&sent);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            if first_attempt_for_sender.swap(false, Ordering::SeqCst) {
                return Err(std::io::Error::other("synthetic Pi write failure"));
            }
            sent_for_sender
                .lock()
                .expect("sent lock")
                .push((id, result));
            Ok(())
        });
        let broker = PermissionBroker::for_test(sender);
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(58, permission("send-failure"), &runtime)
            .expect("register");

        assert!(broker
            .auto_answer("send-failure", &runtime)
            .expect("fallback cancellation"));
        assert_eq!(broker.pending_len(), 0);
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent[0].1["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn ask_mode_leaves_permission_request_for_the_broker() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("grok".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "ask".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(54, permission("ask"), &runtime)
            .expect("register");

        assert!(!broker.auto_answer("ask", &runtime).expect("ask policy"));
        assert_eq!(broker.pending_len(), 1);
    }

    #[test]
    fn auto_accept_prefers_allow_once_then_allow_always() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("grok".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "auto_accept".to_string(),
                available_modes: Vec::new(),
            }),
        });
        broker
            .register(
                55,
                permission_with_kinds("auto-accept", &[("always", "allow_always")]),
                &runtime,
            )
            .expect("register");

        assert!(broker.auto_answer("auto-accept", &runtime).expect("answer"));
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent[0].1["outcome"]["optionId"], "always");
    }

    #[test]
    fn invalid_response_interleaving_preserves_new_registration() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                52,
                permission_with_kinds("reused", &[("always", "allow_always")]),
                &runtime,
            )
            .expect("old register");
        let broker_for_hook = Arc::downgrade(&broker);
        let runtime_for_hook = Arc::clone(&runtime);
        broker.set_after_take_hook(Arc::new(move || {
            broker_for_hook
                .upgrade()
                .expect("broker")
                .register(53, permission("reused"), &runtime_for_hook)
                .expect("new registration");
        }));

        let error = broker
            .respond("reused", PermissionOutcome::Deny)
            .expect_err("old request has no one-shot deny option");
        assert!(error
            .to_string()
            .contains("did not offer any reject option"));
        broker
            .respond("reused", PermissionOutcome::AllowOnce)
            .expect("new registration remains answerable");

        let sent = sent.lock().expect("sent lock");
        assert!(sent
            .iter()
            .any(|(id, result)| { *id == 52 && result["outcome"]["outcome"] == "cancelled" }));
        assert!(sent
            .iter()
            .any(|(id, result)| { *id == 53 && result["outcome"]["optionId"] == "allow" }));
    }

    #[test]
    fn broker_rejects_permission_floods_at_the_per_session_limit() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        for index in 0..MAX_PENDING_ACP_PERMISSIONS {
            broker
                .register(
                    index as u64,
                    permission(&format!("flood-{index}")),
                    &runtime,
                )
                .expect("within limit");
        }
        let error = match broker.register(999, permission("flood-over-limit"), &runtime) {
            Ok(_) => panic!("limit must reject another request"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("maximum"));
        assert_eq!(broker.pending_len(), MAX_PENDING_ACP_PERMISSIONS);
    }

    #[test]
    fn permission_request_rejects_more_than_256_args() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        let mut event = permission("too-many-args");
        match &mut event {
            SessionEvent::PermissionRequest { args, .. } => {
                *args = Some(
                    (0..=MAX_ACP_PERMISSION_ARGS)
                        .map(|index| format!("a{index}"))
                        .collect(),
                );
            }
            _ => panic!("permission fixture is a PermissionRequest"),
        }
        let error = match broker.register(1, event, &runtime) {
            Ok(_) => panic!("{} args must be rejected", MAX_ACP_PERMISSION_ARGS + 1),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains(&MAX_ACP_PERMISSION_ARGS.to_string()),
            "rejection must name the arg cap: {error}"
        );
    }
    /// §8b A14: the origin the broker stamps is what the card's provenance
    /// line renders. A peer session's card names its device; a session whose
    /// origin the registry never installed says `unknown`, which the app draws
    /// as a line that does not claim to be this machine. `local` — drawn as no
    /// line at all — is only ever what a *measured* local session gets.
    #[test]
    fn a_registered_request_carries_the_sessions_origin() {
        let (broker, _sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.set_origin(SessionOrigin::peer("device-phone", PeerRole::Client));
        broker
            .register(1, permission("origin-peer"), &runtime)
            .expect("register");
        let pending = broker.take("origin-peer", None).expect("pending");
        match &pending.request {
            SessionEvent::PermissionRequest { origin, .. } => assert_eq!(
                origin,
                &SessionOrigin::peer("device-phone", PeerRole::Client)
            ),
            other => panic!("permission fixture is a PermissionRequest: {other:?}"),
        }

        // A runtime the registry never told is *not* this machine's: the
        // broker stamps what `SessionRuntime::origin()` holds, and that is
        // `unknown` until the registry measures the session. `local` here
        // would be invented provenance — which is exactly what this test now
        // refuses to accept.
        let unstored_runtime = Arc::new(SessionRuntime::new());
        broker
            .register(2, permission("origin-unstored"), &unstored_runtime)
            .expect("register");
        let pending = broker.take("origin-unstored", None).expect("pending");
        match &pending.request {
            SessionEvent::PermissionRequest { origin, .. } => {
                assert_eq!(origin, &SessionOrigin::unknown());
                assert!(!origin.is_local());
            }
            other => panic!("permission fixture is a PermissionRequest: {other:?}"),
        }
    }

    /// A device may hold three undecided cards; the fourth is refused rather
    /// than stacked in the queue the person at this machine has to read.
    ///
    /// The device id is this test's own: the allowance is now daemon-wide, so
    /// a shared id would let two tests running in parallel spend each other's
    /// slots.
    #[test]
    fn a_peer_may_hold_three_permission_cards_and_not_four() {
        let (broker, _sent) = test_broker();
        let device = "device-three-cards";
        let runtime = Arc::new(SessionRuntime::new());
        runtime.set_origin(SessionOrigin::peer(device, PeerRole::Client));
        for index in 0..MAX_PENDING_FOR_PEER {
            broker
                .register(index as u64, permission(&format!("card-{index}")), &runtime)
                .expect("a card inside the allowance");
        }
        assert_eq!(peer_card_count(device), MAX_PENDING_FOR_PEER);
        // `expect_err` would need `PendingPermission: Debug`, and the type holds
        // a responder that is not printable on purpose; a match is the honest
        // shape here.
        let error = match broker.register(9, permission("card-over"), &runtime) {
            Ok(_) => panic!("the fourth card for one device must be refused"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains(&MAX_PENDING_FOR_PEER.to_string()),
            "the refusal names the allowance: {error}"
        );
        assert_eq!(broker.pending_len(), MAX_PENDING_FOR_PEER);
        assert_eq!(
            peer_card_count(device),
            MAX_PENDING_FOR_PEER,
            "a refusal must not spend a slot"
        );
        // Closing the session the cards belonged to gives all three back.
        broker.close();
        assert_eq!(peer_card_count(device), 0);
    }

    /// H2: the allowance is the *device's*, so two sessions of one paired
    /// device share it. Three cards across two sessions, and the fourth is
    /// refused whichever session asks for it.
    #[test]
    fn one_peer_holds_three_cards_across_two_sessions_and_not_four() {
        let device = "device-two-sessions";
        let (first, _) = test_broker();
        let (second, _) = test_broker();
        let first_runtime = Arc::new(SessionRuntime::new());
        first_runtime.set_origin(SessionOrigin::peer(device, PeerRole::Client));
        let second_runtime = Arc::new(SessionRuntime::new());
        second_runtime.set_origin(SessionOrigin::peer(device, PeerRole::Client));

        // Two cards in one session, one in the other: the device is full.
        for index in 0..2u64 {
            first
                .register(index, permission(&format!("first-{index}")), &first_runtime)
                .expect("a card inside the device's allowance");
        }
        second
            .register(2, permission("second-0"), &second_runtime)
            .expect("the third card of the same device");
        assert_eq!(peer_card_count(device), MAX_PENDING_FOR_PEER);

        for (broker, runtime, label) in [
            (&first, &first_runtime, "the first session"),
            (&second, &second_runtime, "the second session"),
        ] {
            let error = match broker.register(9, permission("over"), runtime) {
                Ok(_) => panic!("{label} must not get a fourth card for this device"),
                Err(error) => error,
            };
            assert!(
                error
                    .to_string()
                    .contains(&MAX_PENDING_FOR_PEER.to_string()),
                "{label}: {error}"
            );
        }

        // A second session closing releases the one card it held, and the
        // device is back inside its allowance.
        second.close();
        assert_eq!(peer_card_count(device), 2);
        first
            .register(9, permission("after-close"), &first_runtime)
            .expect("the slot the closed session held is available again");
        assert_eq!(peer_card_count(device), MAX_PENDING_FOR_PEER);
        first.close();
        assert_eq!(peer_card_count(device), 0);
    }

    /// `release_peer_cards` is the disconnect half: the device is gone, so
    /// nothing it left pending may hold a slot for the rest of the process.
    #[test]
    fn a_disconnected_peer_gives_every_slot_back() {
        let device = "device-disconnect";
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        runtime.set_origin(SessionOrigin::peer(device, PeerRole::Client));
        broker
            .register(1, permission("pending-card"), &runtime)
            .expect("one card");
        assert_eq!(peer_card_count(device), 1);
        super::release_peer_cards(device);
        assert_eq!(peer_card_count(device), 0);
        // The card itself is still pending for the person at this machine.
        assert_eq!(broker.pending_len(), 1);
        broker.close();
        assert_eq!(
            peer_card_count(device),
            0,
            "closing a session whose slots were already released must not underflow"
        );
    }

    /// The allowance is a peer's, not the local person's: a local session may
    /// still queue more than three, which is what the desktop has always done.
    #[test]
    fn a_local_session_is_not_under_the_per_peer_allowance() {
        let (broker, _sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        for index in 0..MAX_PENDING_FOR_PEER + 1 {
            broker
                .register(
                    index as u64,
                    permission(&format!("local-{index}")),
                    &runtime,
                )
                .expect("a local session is not capped per peer");
        }
        assert_eq!(broker.pending_len(), MAX_PENDING_FOR_PEER + 1);
    }

    #[test]
    fn two_permission_requests_correlate_independently() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(11, permission("first"), &runtime)
            .expect("first");
        broker
            .register(12, permission("second"), &runtime)
            .expect("second");
        broker
            .respond("second", PermissionOutcome::Deny)
            .expect("second response");
        broker
            .respond("first", PermissionOutcome::AllowOnce)
            .expect("first response");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0, 12);
        assert_eq!(sent[0].1["outcome"]["optionId"], "deny");
        assert_eq!(sent[1].0, 11);
        assert_eq!(sent[1].1["outcome"]["optionId"], "allow");
    }

    #[test]
    fn permission_response_races_timeout_with_one_journaled_reply() {
        let path = permission_path("race");
        let journal = Arc::new(Journal::open(&path).expect("journal"));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let recorded_before_send = Arc::new(AtomicBool::new(false));
        let sender_started = Arc::new(Barrier::new(2));
        let sender_release = Arc::new(Barrier::new(2));
        let path_for_sender = path.clone();
        let sent_for_sender = Arc::clone(&sent);
        let recorded_for_sender = Arc::clone(&recorded_before_send);
        let entered_for_sender = Arc::clone(&sender_started);
        let release_for_sender = Arc::clone(&sender_release);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            let conn = Connection::open(&path_for_sender).expect("inspect journal");
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM permissions WHERE session_id = ?1 AND request_id = ?2",
                    ["s.permission.race", "race"],
                    |row| row.get(0),
                )
                .expect("permission row count");
            recorded_for_sender.store(count == 1, Ordering::Release);
            sent_for_sender
                .lock()
                .expect("sent lock")
                .push((id, result));
            entered_for_sender.wait();
            release_for_sender.wait();
            Ok(())
        });
        let broker = PermissionBroker::for_test(sender);
        let runtime = Arc::new(SessionRuntime::for_acp(
            "s.permission.race".to_string(),
            Some(Arc::clone(&journal)),
            Arc::clone(&broker),
        ));
        let pending = broker
            .register(21, permission("race"), &runtime)
            .expect("register");
        let start = Arc::new(Barrier::new(3));
        let respond_broker = Arc::clone(&broker);
        let respond_start = Arc::clone(&start);
        let respond_thread = thread::spawn(move || {
            respond_start.wait();
            respond_broker.respond("race", PermissionOutcome::AllowOnce)
        });
        let expire_broker = Arc::clone(&broker);
        let expire_start = Arc::clone(&start);
        let expire_thread = thread::spawn(move || {
            expire_start.wait();
            expire_broker.expire("race", &pending)
        });
        start.wait();
        sender_started.wait();
        sender_release.wait();
        let _ = respond_thread.join().expect("respond thread");
        let _ = expire_thread.join().expect("expiry thread");

        journal.flush().expect("journal flush");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.iter().filter(|(id, _)| *id == 21).count(), 1);
        assert_eq!(sent.len(), 1);
        assert!(recorded_before_send.load(Ordering::Acquire));
        journal.shutdown();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn daemon_shutdown_cancels_outstanding_request_before_reconnect() {
        let (old, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        old.register(31, permission("dead"), &runtime)
            .expect("register");
        old.close();
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(old.pending_len(), 0);
        drop(old);
    }

    #[test]
    fn cancel_pending_completes_but_leaves_the_broker_open() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(51, permission("soft-stop"), &runtime)
            .expect("register");
        broker.cancel_pending();
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(broker.pending_len(), 0);
        broker
            .register(52, permission("after-stop"), &runtime)
            .expect("a soft interrupt must not close the broker");
    }

    #[test]
    fn close_completes_and_rejects_later_requests() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(61, permission("closing"), &runtime)
            .expect("register");
        broker.close();
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(broker.pending_len(), 0);
        let error = match broker.register(62, permission("too-late"), &runtime) {
            Ok(_) => panic!("a closed broker must reject new requests"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("closed"), "{error}");
    }

    /// C6, the mutation that keeps the typestate honest: a validation
    /// failure on the delegated path refuses and leaves the card pending —
    /// it never routes into `respond`, which would complete the card as
    /// cancelled and tell the child its answer was cancelled.
    #[test]
    fn the_delegated_path_never_routes_an_unsupported_outcome_into_respond() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                91,
                permission_with_kinds(
                    "mixed",
                    &[("always", "allow_always"), ("reject", "reject_once")],
                ),
                &runtime,
            )
            .expect("register");

        let error = broker
            .answer_delegated(
                "mixed",
                PermissionOutcome::AllowOnce,
                &|| true,
                &|_| false,
                &|_| Ok(()),
                &|_| Ok(()),
                "s.creator",
            )
            .expect_err("the card offers no one-shot allow");
        assert!(error.contains("no one-shot option"), "{error}");
        assert_eq!(broker.pending_len(), 1, "the card stays pending");
        assert!(
            sent.lock().expect("sent lock").is_empty(),
            "nothing — not even a cancellation — went to the agent"
        );

        // The supported answer on the same card, once the check passes:
        // deny maps to the one-shot reject option and resolves.
        broker
            .answer_delegated(
                "mixed",
                PermissionOutcome::Deny,
                &|| true,
                &|_| false,
                &|_| Ok(()),
                &|_| Ok(()),
                "s.creator",
            )
            .expect("deny has a one-shot reject");
        assert_eq!(broker.pending_len(), 0);
    }

    #[test]
    fn duplicate_or_conflicting_responses_are_rejected() {
        let (broker, _) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(41, permission("once"), &runtime)
            .expect("register");
        broker
            .respond("once", PermissionOutcome::AllowOnce)
            .expect("first response");
        assert!(matches!(
            broker.respond("once", PermissionOutcome::AllowOnce),
            Err(super::PermissionResponseError::NotFound)
        ));
        assert!(matches!(
            broker.respond("once", PermissionOutcome::Deny),
            Err(super::PermissionResponseError::NotFound)
        ));
    }
}

#[cfg(test)]
mod question_tests {
    use super::{
        permission_path, permission_question, permission_question_single_option, test_broker,
        PermissionBroker, PermissionSender,
    };
    use crate::journal::Journal;
    use crate::session::SessionRuntime;
    use devboule_protocol::{PermissionOutcome, SessionEvent};
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    fn manifest_with_mode(runtime: &SessionRuntime, mode_id: &str) {
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: mode_id.to_string(),
                available_modes: Vec::new(),
            }),
        });
    }

    /// A question is never auto-answered, in any mode — including bypass
    /// and "may run without asking". The gate is semantic (`kind`), not
    /// the option-kind heuristic: the registry holds a single-option
    /// question here, which no repeated kind marks as a chooser, so only
    /// the `kind` refusal can keep it pending.
    #[test]
    fn question_is_never_auto_answered_in_any_mode() {
        for (index, mode_id) in [
            "bypass",
            "auto_accept",
            "bypassPermissions",
            "default",
            "acceptEdits",
        ]
        .iter()
        .enumerate()
        {
            let (broker, sent) = test_broker();
            let runtime = Arc::new(SessionRuntime::new());
            manifest_with_mode(&runtime, mode_id);
            let tool_call_id = format!("question-{mode_id}");
            broker
                .register(
                    200 + index as u64,
                    permission_question_single_option(&tool_call_id),
                    &runtime,
                )
                .expect("register");
            assert!(
                !broker
                    .auto_answer(&tool_call_id, &runtime)
                    .expect("auto-answer policy"),
                "a question must stay pending in mode {mode_id}"
            );
            assert_eq!(broker.pending_len(), 1);
            assert!(sent.lock().expect("sent lock").is_empty());
        }
    }

    #[test]
    fn question_option_pick_reports_the_option_id() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(210, permission_question("question-pick"), &runtime)
            .expect("register");
        broker
            .respond_with_option(
                "question-pick",
                PermissionOutcome::AllowOnce,
                Some("q0o1".to_string()),
                None,
            )
            .expect("option pick");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1["outcome"]["outcome"], "selected");
        assert_eq!(sent[0].1["outcome"]["optionId"], "q0o1");
        assert!(sent[0].1["outcome"].get("answer").is_none());
        assert_eq!(broker.pending_len(), 0);
    }

    #[test]
    fn question_grant_without_pick_or_text_stays_pending() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(211, permission_question("question-empty-grant"), &runtime)
            .expect("register");
        let error = broker
            .respond_with_option(
                "question-empty-grant",
                PermissionOutcome::AllowOnce,
                None,
                None,
            )
            .expect_err("a grant must name what was picked");
        assert!(error.to_string().contains("must name the picked option"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    #[test]
    fn question_refuses_both_carriers_at_once() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(212, permission_question("question-both"), &runtime)
            .expect("register");
        let error = broker
            .respond_with_option(
                "question-both",
                PermissionOutcome::AllowOnce,
                Some("q0o0".to_string()),
                Some("Forest green".to_string()),
            )
            .expect_err("one carrier per answer");
        assert!(error.to_string().contains("not both"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    #[test]
    fn question_dismissal_without_an_option_cancels_cleanly() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(213, permission_question("question-dismiss"), &runtime)
            .expect("register");
        broker
            .respond_with_option("question-dismiss", PermissionOutcome::Deny, None, None)
            .expect("dismissal is an answer");
        let sent = sent.lock().expect("sent lock");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1["outcome"]["outcome"], "cancelled");
        assert_eq!(broker.pending_len(), 0);
    }

    /// A parent agent never answers its child's question — even the
    /// single-option shape no chooser rule marks. The delegation door has
    /// no option id and no text to give, so anything sent here would answer
    /// what nobody chose; the card stays pending for the person.
    #[test]
    fn delegation_refuses_a_question_card() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(
                216,
                permission_question_single_option("question-delegated"),
                &runtime,
            )
            .expect("register");
        let error = broker
            .answer_delegated(
                "question-delegated",
                PermissionOutcome::AllowOnce,
                &|| true,
                &|_| false,
                &|_| Ok(()),
                &|_| Ok(()),
                "s.creator",
            )
            .expect_err("a question stays pending for the person");
        assert!(error.contains("is a question"), "{error}");
        assert_eq!(broker.pending_len(), 1, "the card stays pending");
        assert!(
            sent.lock().expect("sent lock").is_empty(),
            "nothing went to the agent"
        );
    }

    #[test]
    fn tool_request_refuses_a_text_answer() {
        let (broker, sent) = test_broker();
        let runtime = Arc::new(SessionRuntime::new());
        broker
            .register(214, super::permission("tool-with-answer"), &runtime)
            .expect("register");
        let error = broker
            .respond_with_option(
                "tool-with-answer",
                PermissionOutcome::AllowOnce,
                None,
                Some("not a question".to_string()),
            )
            .expect_err("tools have no text door");
        assert!(error.to_string().contains("only a question"));
        assert_eq!(broker.pending_len(), 1);
        assert!(sent.lock().expect("sent lock").is_empty());
    }

    /// The free-text answer reaches the provider's reply frame through a
    /// store that holds none of its words: the `permissions` row keeps the
    /// request plus `allow_once`, and the resolved event carries the
    /// granted kind with no option id or name.
    #[test]
    fn question_free_text_answer_is_journaled_without_its_words() {
        let secret = "chartreuse, the fence nobody else has";
        let path = permission_path("question-answer");
        let _ = std::fs::remove_file(&path);
        let journal = Arc::new(Journal::open(&path).expect("journal"));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sent_for_sender = Arc::clone(&sent);
        let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
            sent_for_sender
                .lock()
                .expect("sent lock")
                .push((id, result));
            Ok(())
        });
        let broker = PermissionBroker::with_sender(sender);
        let runtime = SessionRuntime::for_acp(
            "s.permission.question-answer".to_string(),
            Some(Arc::clone(&journal)),
            Arc::clone(&broker),
        );
        let pending = broker
            .register(215, permission_question("question-answer"), &runtime)
            .expect("register");
        broker
            .respond_with_option(
                "question-answer",
                PermissionOutcome::AllowOnce,
                None,
                Some(secret.to_string()),
            )
            .expect("free-text answer");
        assert_eq!(
            sent.lock().expect("sent lock")[0].1["outcome"]["answer"],
            secret
        );
        // The resolved event for a text answer names no option, but still
        // carries the granted kind: observers read an allowed question as
        // allowed, while no field carries the person's words to any
        // subscriber.
        let resolved = super::permission_resolved_event(&pending, None, "allow_once", None);
        match resolved {
            SessionEvent::PermissionResolved {
                selected_option_id,
                selected_option_kind,
                selected_option_name,
                ..
            } => {
                assert_eq!(selected_option_id, None);
                assert_eq!(selected_option_kind.as_deref(), Some("allow_once"));
                assert_eq!(selected_option_name, None);
            }
            _ => panic!("expected a permission resolution"),
        }
        journal.flush().expect("journal flush");
        let conn = Connection::open(&path).expect("inspect journal");
        let (outcome, payload): (String, Vec<u8>) = conn
            .query_row(
                "SELECT outcome, payload FROM permissions WHERE session_id = ?1 AND request_id = ?2",
                ["s.permission.question-answer", "question-answer"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("permission row");
        assert_eq!(outcome, "allow_once");
        let payload = String::from_utf8_lossy(&payload);
        assert!(
            !payload.contains(secret),
            "the journal payload must not hold the person's answer"
        );
        assert!(
            payload.contains("Which colour should I paint the fence?"),
            "the journal payload still holds the question itself: {payload}"
        );
        drop(conn);
        journal.shutdown();
        let _ = std::fs::remove_file(path);
    }
}

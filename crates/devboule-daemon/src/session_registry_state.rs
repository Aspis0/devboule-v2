//! The registry's standing state: the type aliases and caches it holds, the
//! presence it records, the message brake and its table, and the agent creation
//! table with the guards, tickets and records one creation carries.
//!
//! Split out of `session_items.rs` without a rewrite: every line below this
//! header is byte-identical to its text there (lines 1316-2268 of `7235ff8`,
//! the file's tail after `prompt_text_with_fallback_paths`), and not one
//! visibility marker changed - every type keeps the `pub`, `pub(crate)` or
//! `pub(super)` it had, and a sibling under `session` reaches each of them
//! exactly as far as before.

use super::*;

pub(super) type TransitionSink = Arc<dyn Fn(OwnerId) + Send + Sync>;
pub(super) type JournalRosterCache = Arc<Mutex<Option<(u64, Vec<SessionRecord>)>>>;

/// One client answer to a pending permission request.
pub struct PermissionResponse<'a> {
    pub session_id: &'a str,
    pub request_id: &'a str,
    pub outcome: PermissionOutcome,
    pub option_id: Option<&'a str>,
}

pub(super) const WORKSPACE_PATH_CACHE_CAP: usize = 1024;

#[derive(Default)]
pub(super) struct WorkspacePathCache {
    pub(super) entries: HashMap<String, (PathBuf, u64)>,
    clock: u64,
}

impl WorkspacePathCache {
    fn next_stamp(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }

    pub(super) fn get(&mut self, workspace_id: &str) -> Option<PathBuf> {
        let path = self
            .entries
            .get(workspace_id)
            .map(|(path, _)| path.clone())?;
        let stamp = self.next_stamp();
        self.entries
            .insert(workspace_id.to_string(), (path.clone(), stamp));
        Some(path)
    }

    pub(super) fn insert(&mut self, workspace_id: String, path: PathBuf) {
        if self.entries.len() >= WORKSPACE_PATH_CACHE_CAP
            && !self.entries.contains_key(&workspace_id)
        {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, stamp))| *stamp)
                .map(|(id, _)| id.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        let stamp = self.next_stamp();
        self.entries.insert(workspace_id, (path, stamp));
    }

    pub(super) fn remove(&mut self, workspace_id: &str) {
        self.entries.remove(workspace_id);
    }
}

#[cfg(test)]
pub(super) type JournalRosterAfterListHook = Arc<dyn Fn() + Send + Sync>;

/// Runs between the brake admission and the delivery of an agent message (S4-10).
/// Test-only: it is the only way to land a turn's end inside that gap.
#[cfg(test)]
pub(super) type AgentMessageAfterAdmissionHook = Arc<dyn Fn() + Send + Sync>;

/// Runs between a deposit's ownership check and the store write (HND-01).
/// Test-only: it is the only way to land a close inside that gap.
#[cfg(test)]
pub(super) type DepositAfterOwnershipHook = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub(super) struct ConnectionPresence {
    pub(super) user: String,
    pub(super) focused_session_id: Option<String>,
    pub(super) app_visible: bool,
}

pub(crate) struct MessageBrake {
    pub(super) outstanding: Vec<OutstandingMessage>,
    pub(super) recipients: Vec<Recipient>,
    pub(super) next_slot: u64,
    pub(super) window_started: Instant,
    pub(super) sent_in_window: u32,
}

/// One message that was admitted and has not reached its boundary yet.
pub(super) struct OutstandingMessage {
    pub(super) slot: u64,
    pub(super) sent_at: Instant,
    /// The session this message was sent to: the target whose turn end releases
    /// the slot, and the name the recipient window counts.
    pub(super) to_session: String,
    /// Set once the delivery has returned — the text is in the provider's hands,
    /// or the delivery failed. A boundary that is already reached releases the
    /// slot as soon as this is set.
    pub(super) delivered: bool,
    /// Set when the boundary arrived while the delivery was still in flight.
    ///
    /// The slot stays counted until then: releasing it at the boundary would let
    /// the next message through while this one is still being written, which is
    /// exactly what the outstanding count is there to prevent (A2-05).
    pub(super) boundary_reached: bool,
    /// Where this slot's release arrives: the target runtime whose turn end
    /// releases it, and the id of the one-shot hook registered on it. `None`
    /// once the hook has fired or been unregistered again.
    pub(super) release: Option<(Weak<SessionRuntime>, u64)>,
}

/// One recipient inside the sliding window.
pub(super) struct Recipient {
    pub(super) session_id: String,
    pub(super) sent_at: Instant,
}

/// At most this many messages may be in flight from one sender.
pub(super) const MAX_MESSAGE_OUTSTANDING: usize = 5;
/// At most this many distinct recipients may be reached inside the recipient
/// window.
pub(super) const MAX_MESSAGE_RECIPIENTS: usize = 3;
/// At most this many messages may leave one sender inside the rate window.
pub(super) const MAX_MESSAGE_SENT_PER_WINDOW: u32 = 5;
/// The rate window: the brief's one second, unchanged by this fix.
pub(super) const MESSAGE_RATE_WINDOW: Duration = Duration::from_secs(1);
/// How long one in-flight message may hold a sender's slot, and how long a
/// recipient stays inside the recipient window. A target that never ends a turn
/// — or never starts one — must not park a sender's budget forever.
pub(super) const MESSAGE_SLOT_EXPIRY: Duration = Duration::from_secs(60);

impl MessageBrake {
    pub(super) fn new() -> Self {
        Self {
            outstanding: Vec::new(),
            recipients: Vec::new(),
            next_slot: 1,
            window_started: Instant::now(),
            sent_in_window: 0,
        }
    }

    /// Drop the slots that are over and the recipients that have aged out of the
    /// window, measured against `now`, answering the hooks that were armed for
    /// slots the expiry just ended.
    ///
    /// The expiry is the backstop for a slot whose *delivery* never returns — a
    /// write wedged in a provider's pipe must not park a sender's budget forever
    /// (S4-03) — so it ends the slot whether or not the delivery came back. The
    /// *boundary* (the target's turn ending) is the one that waits for the
    /// delivery, because there the message is still on its way (A2-05).
    ///
    /// A recipient, by contrast, is time-bounded (S4-01): it stays in the window
    /// for [`MESSAGE_SLOT_EXPIRY`] after its last send, whether or not a slot for
    /// it is still in flight, because the window is the fan-out brake — how many
    /// *different* agents one sender has reached lately — and a set that emptied
    /// itself as slots retired would let a sender rotate through targets instead.
    pub(super) fn prune(&mut self, now: Instant) -> Vec<(Weak<SessionRuntime>, u64)> {
        let mut expired: Vec<(Weak<SessionRuntime>, u64)> = Vec::new();
        let mut live: Vec<OutstandingMessage> = Vec::with_capacity(self.outstanding.len());
        for mut slot in self.outstanding.drain(..) {
            if now.saturating_duration_since(slot.sent_at) < MESSAGE_SLOT_EXPIRY {
                live.push(slot);
            } else if let Some((runtime, hook)) = slot.release.take() {
                expired.push((runtime, hook));
            }
        }
        self.outstanding = live;
        // Written out rather than called as a method so the closure borrows only
        // `outstanding` and `recipients`' own `sent_at`, which cannot conflict.
        self.recipients.retain(|recipient| {
            now.saturating_duration_since(recipient.sent_at) < MESSAGE_SLOT_EXPIRY
                || self
                    .outstanding
                    .iter()
                    .any(|entry| entry.to_session == recipient.session_id)
        });
        expired
    }

    pub(super) fn holds_recipient(&self, session_id: &str) -> bool {
        self.recipients
            .iter()
            .any(|recipient| recipient.session_id == session_id)
    }

    /// Remove one slot, answering with its still-armed hook so the caller can
    /// unregister it.
    pub(super) fn take_slot(&mut self, slot: u64) -> Option<(Weak<SessionRuntime>, u64)> {
        let index = self
            .outstanding
            .iter()
            .position(|entry| entry.slot == slot)?;
        self.outstanding.remove(index).release
    }

    /// Whether an outstanding message still names this session (A2-06).
    fn has_slot_for(&self, session_id: &str) -> bool {
        self.outstanding
            .iter()
            .any(|entry| entry.to_session == session_id)
    }

    /// Drop one recipient once it has nothing in flight *and* has aged out of
    /// the window (S4-01).
    ///
    /// A recipient younger than [`MESSAGE_SLOT_EXPIRY`] stays, even when its last
    /// slot is gone: the window is the fan-out brake, and dropping the entry the
    /// moment a slot retires would let a sender reach an unbounded number of
    /// agents by rotating through them.
    pub(super) fn drop_recipient_if_idle(&mut self, session_id: &str, now: Instant) {
        if self.has_slot_for(session_id) || self.recipient_in_window(session_id, now) {
            return;
        }
        self.recipients
            .retain(|recipient| recipient.session_id != session_id);
    }

    /// Whether this session is still inside the recipient window (S4-01).
    fn recipient_in_window(&self, session_id: &str, now: Instant) -> bool {
        self.recipients.iter().any(|recipient| {
            recipient.session_id == session_id
                && now.saturating_duration_since(recipient.sent_at) < MESSAGE_SLOT_EXPIRY
        })
    }

    /// Nothing left to remember: the sender's entry can leave the table.
    pub(super) fn is_idle(&self) -> bool {
        self.outstanding.is_empty() && self.recipients.is_empty()
    }
}

/// The agent-message brakes, with the clock of the last global sweep (S4-16).
///
/// One mutex covers both: an admission that sweeps and an admission that reserves
/// cannot interleave half-way, and the sweep cannot run more often than
/// [`MESSAGE_RATE_WINDOW`] no matter how many senders are active. Every access
/// site still reads the map directly through `Deref`, so the entries and the
/// sweep clock cannot drift apart.
#[derive(Default)]
pub(crate) struct MessageBrakeTable {
    entries: HashMap<String, MessageBrake>,
    last_sweep: Option<Instant>,
    /// How many sweeps actually ran (S4-16). Test-only: the cadence is otherwise
    /// invisible from outside the table.
    #[cfg(test)]
    pub(super) sweeps: u64,
}

impl std::ops::Deref for MessageBrakeTable {
    type Target = HashMap<String, MessageBrake>;

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl std::ops::DerefMut for MessageBrakeTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entries
    }
}

impl MessageBrakeTable {
    /// Whether the global sweep may run now (S4-16).
    ///
    /// The sweep walks every other sender's entry while this single lock is held,
    /// so it is a per-window cost rather than a per-send one. The caller's own
    /// entry is still pruned on every reserve, which is what its own braking
    /// needs; the sweep only bounds the table.
    pub(super) fn sweep_is_due(&self, now: Instant) -> bool {
        self.last_sweep
            .is_none_or(|last| now.saturating_duration_since(last) >= MESSAGE_RATE_WINDOW)
    }

    pub(super) fn note_sweep(&mut self, now: Instant) {
        self.last_sweep = Some(now);
        #[cfg(test)]
        {
            self.sweeps = self.sweeps.saturating_add(1);
        }
    }
}

pub(crate) struct LiveAgentEntry {
    pub(crate) session: Session,
    pub(crate) runtime: Arc<SessionRuntime>,
}

/// At most this many live children may one creator session hold at once
/// (`S5` decision 5).
pub(crate) const MAX_LIVE_CHILDREN_PER_CREATOR: usize = 3;
/// At most this many creations may leave one creator session inside the window.
pub(crate) const MAX_CREATIONS_PER_WINDOW: u32 = 10;
/// The creation window: one hour, from the first creation that opened it.
pub(crate) const CREATION_WINDOW: Duration = Duration::from_secs(60 * 60);
/// The deepest a created agent may be. A child of a child is depth 2; a
/// session at depth 2 may not create (`S5` decision 5).
pub(crate) const MAX_AGENT_DEPTH: u32 = 2;
/// At most this many agent-created sessions may be live in the whole daemon.
pub(crate) const MAX_LIVE_AGENT_SESSIONS: usize = 8;
/// Largest artifact one finish report deposits (32 KiB).
///
/// A cap, not a target: a child's last message is usually a few hundred bytes,
/// and the whole message — not the truncated summary — is what is stored. A
/// message over this is reported with a note instead, which is the same shape
/// as a deposit the store refused.
pub(crate) const MAX_AGENT_ARTIFACT_BYTES: usize = 32 * 1024;
/// How long one in-flight creation holds its idempotency key (`S5-03`).
///
/// Long enough for a card a human answers and a provider handshake behind it;
/// short enough that a thread which died mid-creation cannot make a key
/// permanently unusable.
pub(crate) const CREATION_PENDING_TTL: Duration = Duration::from_secs(5 * 60);

/// How long a parked child end, or a pending-creation marker, can belong to a
/// live creation: the slot's own expiry. Past it, [`AgentCreationTable::sweep`]
/// drops them (audit-3 §2) — a creation slower than this is a thread that died,
/// not a provider still starting.
pub(crate) const DEFERRED_SLOT_EXPIRY: Duration = Duration::from_secs(60);

/// The once-per-creator-session creation gate (`S5` decision 4, hardened by
/// audit S5-06).
///
/// Three states, not a bool, because the decision is made *outside* this lock
/// (the human answers a card) and a second caller must not be able to raise a
/// second card while the first one is unanswered: `Closed` has not been asked
/// yet, `Pending` has been asked and no one has answered, `Open` was answered
/// with an allow and stays open for as long as this entry lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CreationGate {
    Closed,
    Pending,
    Open,
}

/// What one creator session's budget currently holds.
pub(super) struct AgentCreatorCaps {
    /// Children that exist.
    pub(super) live_children: usize,
    /// Children this creator has reserved and not yet committed or abandoned:
    /// the slot is taken *before* the card is raised, so two creations racing
    /// on one session cannot both see the third slot free.
    /// The reservations in flight, by id, each naming the child session id it
    /// reserved (audit S5B-02). The id is the identity: releasing one is a
    /// removal that answers whether it was there, so a failure handled on two
    /// paths cannot subtract a neighbour's creation.
    pub(super) in_flight: BTreeMap<u64, String>,
    window_started: Instant,
    pub(super) creations_in_window: u32,
    /// The once-per-creator-session accept (`S5` decision 4, S5-06). It lives
    /// exactly as long as this entry does, and it is read and written only
    /// under this table's lock so two creations racing on one session cannot
    /// both be told to ask.
    pub(super) gate: CreationGate,
    /// Set when the creator session is gone: the entry then lives until its
    /// last child finishes, because that is what releases the daemon-wide
    /// count.
    pub(super) creator_gone: bool,
}

impl AgentCreatorCaps {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            live_children: 0,
            in_flight: BTreeMap::new(),
            window_started: now,
            creations_in_window: 0,
            gate: CreationGate::Closed,
            creator_gone: false,
        }
    }

    /// How many children this creator holds or is about to hold: what the
    /// three-child cap counts.
    pub(super) fn held(&self) -> usize {
        self.live_children + self.in_flight.len()
    }

    /// Roll the window if it has expired. Called on every admission *and* on
    /// the sweep, so the count a caller reads is never one window stale.
    pub(super) fn roll_window(&mut self, now: Instant) {
        if now.saturating_duration_since(self.window_started) >= CREATION_WINDOW {
            self.window_started = now;
            self.creations_in_window = 0;
        }
    }
}

/// One child, as its creator's bookkeeping sees it.
pub(super) struct AgentChild {
    pub(super) creator: String,
    /// Whether the creator asked to be told (the tool's `notifyOnFinish`).
    pub(super) notify: bool,
    /// Whether the session behind this link was actually started (audit
    /// S5B-04). The link is registered when the reservation is taken — before
    /// the spawn — so a child that exits on the instant cannot outrun the row
    /// that catches its end; the child counts against its creator only once
    /// the spawn returned a session.
    pub(super) started: bool,
    /// The `input_required` notice is owed until it has been sent once
    /// (`S5` §3): one notice per child, not one per card.
    pub(super) notice_owed: bool,
    /// The finish report is owed until it has been written once. This is what
    /// makes the report idempotent across the three paths that can observe the
    /// same end (a finished turn, a process exit, a close).
    pub(super) report_owed: bool,
    /// The quiet notice is owed until it has been sent once per quiet spell.
    /// Cleared when the child publishes again, so one spell is one notice.
    pub(super) quiet_notified: bool,
}

/// One parked child end (audit-2 §2): what the end path still had in hand when
/// the child was gone. Any slot may be `None`; a unit test drives that shape,
/// and the type is named so the table and the commit read as one thing.
pub(super) type DeferredChildEnd = (
    Option<Session>,
    Option<Arc<SessionRuntime>>,
    Option<OwnerId>,
);

/// The creation budget, beside [`MessageBrakeTable`] and under the same lock
/// discipline: one mutex covers the whole table, the sweep runs at most once
/// per window, and no other lock is taken while it is held.
#[derive(Default)]
pub(crate) struct AgentCreationTable {
    pub(super) creators: HashMap<String, AgentCreatorCaps>,
    pub(super) children: HashMap<String, AgentChild>,
    /// Children an agent's creation has spawned but not committed yet
    /// (audit-2 §2): their end waits instead of running against a link that
    /// does not exist yet.
    pub(super) pending_children: HashMap<String, (Instant, u64)>,
    /// Ends that arrived while their child was still pending, kept whole
    /// (session view, runtime, owner) so the commit can run the routine the
    /// moment the link exists.
    pub(super) deferred_child_ends: HashMap<String, (DeferredChildEnd, Instant)>,
    /// The idempotency keys of creations that are in flight right now
    /// (audit S5-03): a retry that arrives while its key is here is refused
    /// without spending anything, because the first call has not answered yet.
    pending: HashMap<String, Instant>,
    /// The next reservation id (audit S5B-02). Unique for the life of the
    /// table, which is what makes a release answerable.
    pub(super) next_reservation: u64,
    pub(super) last_sweep: Option<Instant>,
    #[cfg(test)]
    sweeps: u64,
}

impl AgentCreationTable {
    pub(super) fn sweep_is_due(&self, now: Instant) -> bool {
        self.last_sweep
            .is_none_or(|last| now.saturating_duration_since(last) >= CREATION_WINDOW)
    }

    /// Drop the entries that can no longer say anything: a creator whose
    /// session is gone and whose children have all finished.
    ///
    /// The window is rolled unconditionally (every entry, whatever its age) so
    /// a table that is swept once an hour still reports this hour's count.
    pub(super) fn sweep(&mut self, now: Instant) {
        for caps in self.creators.values_mut() {
            caps.roll_window(now);
        }
        self.creators
            .retain(|_, caps| !(caps.creator_gone && caps.held() == 0));
        // The backstop for the parked ends (audit-3 §2): a creation whose thread
        // died, or a creator that closed in the wrong instant, leaves a parked
        // end behind, and past the slot expiry it cannot belong to a live
        // creation any more.
        //
        // `pending_children` is deliberately **not** aged here (audit-3 S5D-01):
        // a marker is the link between an end that arrived early and the commit
        // that has not run yet, and a spawn slower than the expiry — an ACP
        // handshake is not fast — would lose that link here, stranding the
        // reservation and the finish report with it. A marker lives exactly as
        // long as its reservation: the commit and the abandon remove it by name,
        // and `release_agent_creation` removes it with the reservation.
        self.deferred_child_ends
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < DEFERRED_SLOT_EXPIRY);
        self.last_sweep = Some(now);
        #[cfg(test)]
        {
            self.sweeps = self.sweeps.saturating_add(1);
        }
    }

    /// How many agent-created sessions the daemon holds or is about to hold
    /// (audit S5-02): committed children **plus** every reservation that has
    /// not been committed or abandoned yet.
    ///
    /// Counting only `children` let concurrent creators each pass the global
    /// check and then commit past the cap; a reservation is a session the
    /// daemon has already promised to someone, so it is counted from the
    /// moment it is taken.
    pub(super) fn live_agent_sessions(&self) -> usize {
        // Committed children **plus** every reservation still in flight (audit
        // S5-02): a reservation is a session the daemon has already promised,
        // and the two sets are disjoint — a reservation is dropped when its
        // child is committed.
        self.children.len()
            + self
                .creators
                .values()
                .map(|caps| caps.in_flight.len())
                .sum::<usize>()
    }

    /// Claim the idempotency key of a creation that is starting (`S5-03`).
    ///
    /// False means another call with the same key is in flight: that call is
    /// refused before a slot, a card or a session is spent on it. An entry
    /// older than [`CREATION_PENDING_TTL`] is taken over rather than honoured,
    /// because a thread that died mid-creation must not make its key
    /// permanently unusable.
    pub(super) fn begin_creation(&mut self, key: &str, now: Instant) -> bool {
        match self.pending.get(key) {
            Some(started) if now.saturating_duration_since(*started) < CREATION_PENDING_TTL => {
                false
            }
            _ => {
                self.pending.insert(key.to_string(), now);
                true
            }
        }
    }

    /// The creation this key was claimed for is over, either way: its result is
    /// in the idempotency store, or it failed and stored nothing.
    pub(super) fn end_creation(&mut self, key: &str) {
        self.pending.remove(key);
    }
}

/// One creation's hold on its idempotency key (audit S5-03).
///
/// The key is claimed before the idempotency store is read and released when
/// this goes out of scope, so the handler's refusals — a bad workspace, a
/// refused card, a provider that would not spawn — do not each need a release
/// line, and a panic in between cannot wedge the key for good: the mark also
/// expires on its own ([`CREATION_PENDING_TTL`]).
pub(crate) struct CreationKeyHold<'a> {
    pub(super) sessions: &'a SessionRegistry,
    pub(super) key: Option<String>,
}

impl CreationKeyHold<'_> {
    /// The call answered and remembered its result: the key stops being in
    /// flight now, while the idempotency store keeps the answer a retry reads.
    ///
    /// It takes `&mut self` rather than `self` so a caller can commit through
    /// `Option::as_mut` without moving the guard out of it.
    pub(crate) fn commit(&mut self) {
        self.release();
    }

    fn release(&mut self) {
        if let Some(key) = self.key.take() {
            self.sessions.end_agent_creation(&key);
        }
    }
}

impl Drop for CreationKeyHold<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

/// What one reservation answers: the numbers the creation card states, and
/// whether the card is still owed for this creator session.
///
/// It is also the reservation's identity (audit S5B-02) and it releases the
/// reservation when it is dropped, so every refusal and every failure between
/// the reserve and the commit — a bad workspace, a refused card, a spawn that
/// returned an error — gives the slot back exactly once without a release line
/// per path, and a release cannot happen twice.
pub(crate) struct AgentCreationTicket<'a> {
    pub(super) registry: &'a SessionRegistry,
    pub(super) creator: String,
    pub(super) reservation: u64,
    /// The child session id reserved for this creation (audit S5B-04).
    pub(super) child: String,
    pub(super) card_owed: bool,
    pub(super) committed: bool,
    pub(super) caps: devboule_protocol::CreateAgentCaps,
}

impl AgentCreationTicket<'_> {
    pub(crate) fn card_owed(&self) -> bool {
        self.card_owed
    }

    pub(crate) fn caps(&self) -> &devboule_protocol::CreateAgentCaps {
        &self.caps
    }

    pub(crate) fn reservation(&self) -> u64 {
        self.reservation
    }

    /// The slot is now a child: it stops being a reservation, and nothing is
    /// given back when this is dropped.
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for AgentCreationTicket<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.registry
                .release_agent_creation(&self.creator, self.reservation);
        }
    }
}

impl std::fmt::Debug for AgentCreationTicket<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentCreationTicket")
            .field("creator", &self.creator)
            .field("reservation", &self.reservation)
            .field("child", &self.child)
            .field("card_owed", &self.card_owed)
            .field("committed", &self.committed)
            .finish()
    }
}

/// What a create carries beyond the wire's own frame (`S5` §3).
///
/// The human path fills in `display_name` and nothing else. Every other field
/// is written by the daemon for a create an *agent* asked for, and none of them
/// is reachable from `ClientMessage::SessionCreate`: a client cannot name its
/// parent, choose its depth, hand itself a tool overlay, or declare an origin.
#[derive(Default, Clone)]
pub(crate) struct SessionCreateMeta {
    /// Whether this session is an agent's child whose creation has not
    /// committed yet (audit-2 §2). Its end can arrive before the link exists,
    /// so an end with no link is parked rather than reported twice or lost.
    pub(crate) creation_pending: bool,
    /// The reservation whose ticket owns this creation (audit-3 S5D-01). The
    /// spawn notes the child's pending marker with it, so the reservation's
    /// release clears that marker the way the commit and the abandon do: the
    /// marker's life is the reservation's, and the sweep never ages it.
    pub(crate) reservation: Option<u64>,
    /// The id this session must use, when the caller reserved one (audit
    /// S5B-04: an agent's child id is composed by the reservation so the link
    /// can exist before the spawn). `None` means "compose one now", which is
    /// every other caller.
    pub(crate) session_id: Option<String>,
    pub(crate) display_name: Option<String>,
    /// The session that created this one (`None` when a human or a client asked
    /// for it).
    pub(crate) created_by: Option<String>,
    /// How far this session is from a human root: 0 for a human's session, 1
    /// for its child, 2 for a grandchild.
    pub(crate) depth: u32,
    /// The profile's tool overlay, which the broker consults per session.
    pub(crate) overlay: crate::provider_catalog::ToolOverlay,
    /// The origin to record. `None` means "this connection's", which is every
    /// human-started create; a created child passes its creator's stored origin.
    pub(crate) origin: Option<SessionOrigin>,
    /// The working directory in its stored spelling; the hand-off
    /// (`resolve_creation_inputs`) converts it for the child.
    pub(crate) cwd: Option<PathBuf>,
    /// The profile this creation resolved, by its stable id
    /// (`create-from-profile`). `None` for every create that resolved no
    /// profile, which is the human's provider picker and every terminal.
    pub(crate) profile_id: Option<String>,
    /// The labels the creation stamped — the caller's own map plus the daemon's
    /// four `devboule.` keys. Empty for a create that is not an agent's.
    pub(crate) labels: std::collections::BTreeMap<String, String>,
    /// The context this session inherits. `None` means "its own id", which is
    /// every create that is not another session's child; a created child passes
    /// its creator's context, so a creator and everything it commissions share
    /// one at any depth.
    pub(crate) context_id: Option<String>,
}

impl SessionCreateMeta {
    /// What one agent creation carries into the `SessionCreate` path.
    ///
    /// Pure on purpose: this is where the child *inherits*, and the two rules
    /// that matter are readable here in one place. The origin is the creator's
    /// **stored** origin, so a child of a peer's session stays on that peer's
    /// device and with that peer's role (`S5` decision 3: never invented, and
    /// never taken from a connection — an MCP call has no connection). The
    /// creator, the depth and the overlay are the daemon's own facts about the
    /// child, written when the child's MCP registration is made; no parameter of
    /// `devboule_create_agent` reaches any of the four.
    pub(crate) fn for_agent_child(
        creator_session_id: &str,
        origin: &SessionOrigin,
        display_name: &str,
        depth: u32,
        overlay: crate::provider_catalog::ToolOverlay,
        cwd: Option<PathBuf>,
    ) -> Self {
        Self {
            session_id: None,
            creation_pending: true,
            // Written by the creation that holds the ticket, below.
            reservation: None,
            display_name: Some(display_name.to_string()),
            created_by: Some(creator_session_id.to_string()),
            depth,
            overlay,
            origin: Some(origin.clone()),
            cwd,
            // The creation-from-profile facts are written by the creation
            // that resolved a profile, beside the reservation above: this
            // function is the part of a child's birth that does not depend on
            // which profile made it. `context_id: None` here would be "this
            // child is its own context", which is the truth only until the
            // caller puts the creator's context in.
            profile_id: None,
            labels: std::collections::BTreeMap::new(),
            context_id: None,
        }
    }
}

/// The creator's own facts, as a creation reads them (`S5` §3): the child
/// inherits every one of them and invents none.
pub(crate) struct AgentCreator {
    pub(crate) owner: OwnerId,
    pub(crate) origin: SessionOrigin,
    pub(crate) workspace_id: Option<String>,
    pub(crate) display_name: Option<String>,
    pub(crate) title: String,
    /// The context this creator belongs to: its own id, or the context of the
    /// session that created *it*. A child inherits this — that inheritance is
    /// the whole rule, and it is what makes a human's session and every
    /// generation under it one context (`create-from-profile`).
    pub(crate) context_id: String,
}

impl AgentCreator {
    /// Whether the device behind this creator may still create sessions
    /// (`S5` §3).
    ///
    /// A local creator is this daemon's own person: allowed. A peer's creator is
    /// a session that device already created, so its child is a session on that
    /// device and the same capability gate applies to it — judged with the same
    /// `peer_allows` function the dispatcher and the broker door use, on the same
    /// wire message the door names for this tool (`SessionCreate`; that arm reads
    /// only the capability set, so the placeholder kind never decides). The lookup
    /// is fail-closed — an unknown, unreadable or revoked device holds nothing —
    /// and an origin the daemon cannot read (peer-shaped without device or role)
    /// is not a licence either.
    pub(crate) fn may_create_sessions(&self, state: &crate::server::ServerState) -> bool {
        match self.origin.kind {
            SessionOriginKind::Local => true,
            SessionOriginKind::Peer => {
                let (Some(device), Some(role)) =
                    (self.origin.device_id.as_deref(), self.origin.role)
                else {
                    return false;
                };
                let caps = state.peer_caps(device);
                let request = devboule_protocol::ClientMessage::SessionCreate {
                    id: 0,
                    workspace_id: None,
                    kind: SessionKind::Claude,
                    provider: None,
                    mode: None,
                    display_name: None,
                    idempotency_key: None,
                };
                matches!(
                    crate::peer_policy::peer_allows(role, &caps, &request),
                    crate::peer_policy::PeerDecision::Allow
                )
            }
            SessionOriginKind::Unknown => false,
        }
    }
}

impl AgentCreator {
    /// The name to tell the human a creation came from: the creator's display
    /// name when it has one, otherwise its title — the same fallback the app
    /// renders, so the sentence names a row the human can see.
    pub(crate) fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.title)
    }
}

/// One creation an agent asked for.
pub(crate) struct AgentCreation {
    pub(crate) creator_session_id: String,
    pub(crate) creator: AgentCreator,
    /// The creator's runtime, taken *before* the spawn (audit-2 §1): the same
    /// handle the card was raised through. The creation record is published
    /// through it after the spawn, so a lookup that would miss by then cannot
    /// take the record with it.
    pub(crate) creator_runtime: Option<Arc<SessionRuntime>>,
    pub(crate) display_name: String,
    pub(crate) provider: String,
    /// The profile the creation resolved, by its **stable id**: this is what the
    /// session records, so a rename later cannot make a running child misreport
    /// what it was started from.
    pub(crate) profile_id: String,
    /// The profile's name at the moment of the call, which is what the creator's
    /// transcript shows (`SessionEvent::AgentCreated`). A record of a birth: a
    /// rename afterwards does not rewrite it.
    pub(crate) profile_name: String,
    /// The profile's spawn prompt, read when the creation was resolved: the
    /// text the daemon itself puts in front of the child's first prompt,
    /// behind the device's standing instructions and ahead of the creation
    /// preamble and the creator's prompt. Empty means the profile carries
    /// none. It travels down the creation road as data — resolved once, here —
    /// so the send composes from what was resolved and a later edit to the
    /// profile cannot reach a creation already under way.
    pub(crate) spawn_prompt: String,
    /// Everything the profile delivers to the child — the mode, the model, the
    /// thinking option and the `autoAccept` constraint — as one typed value.
    /// The card names all of these; the child is started on all of these or the
    /// creation is refused, so a child that exists was delivered everything its
    /// card printed.
    pub(crate) delivery: crate::profile_delivery::ProfileDelivery,
    pub(crate) overlay: crate::provider_catalog::ToolOverlay,
    /// The labels the child carries: the caller's own plus the four the daemon
    /// stamped.
    pub(crate) labels: std::collections::BTreeMap<String, String>,
    /// The context the child inherits: its creator's.
    pub(crate) context_id: Option<String>,
    pub(crate) depth: u32,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) initial_prompt: String,
    pub(crate) notify: bool,
    /// The workspace the child is created in: the one the caller named, or the
    /// creator's when it named none. Both are the caller's own business to
    /// reach, and the registry resolves the path.
    pub(crate) workspace_id: Option<String>,
}

/// Whether a resolved provider id came from the session-create request
/// or from `DEVBOULE_AGENT_PROVIDER`. Consent for npx wrappers requires
/// the request; the env override cannot supply it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderProvenance {
    Request,
    Env,
}

/// Everything one send needs beyond the registry itself.
///
/// Folded into one value rather than seven positional arguments: the call
/// shape is read in one place, the send path stops growing a parameter per
/// slice, and `server.rs::session_send` builds it from the frame in one
/// literal. `origin` is deliberately not here — it is set once at create,
/// stored on the session, and read from there.
pub struct SendRequest<'a> {
    pub session_id: &'a str,
    pub subscription_id: u64,
    pub text: &'a str,
    pub attachments: &'a [PromptAttachment],
    /// The stored attachments this prompt refers to, beside the inline ones.
    ///
    /// Resolved against `session_id`, not against any session the references
    /// themselves name: the protocol refuses a reference whose session is not
    /// the request's before the store is asked anything, so the two can never
    /// disagree about where a digest resolves. Both are empty in the common
    /// case, which is what keeps a text-only prompt free of every check these
    /// two fields bring.
    pub attachment_references: &'a [AttachmentReference],
    pub owner: &'a OwnerId,
    pub conn: &'a ConnHandle,
    pub mcp_timeout: Duration,
    pub active_turn_behavior: Option<ActiveTurnBehavior>,
    pub require_attachment: bool,
    /// Whether a `Steer` the provider cannot take may fall back to interrupting
    /// the running turn and replacing it (S4-01).
    ///
    /// True for the person at this machine and for a local agent's own message
    /// delivery. False for a paired device: interrupting a turn is the act
    /// `SessionInterrupt` decides, and no capability opens it to a peer, so a
    /// peer's steer must not reach `killer.interrupt()` the long way round.
    pub interrupt_on_steer_refusal: bool,
    /// The brake slot this delivery belongs to, when the send is an agent message
    /// that reserved one (S4-10).
    ///
    /// The plain-prompt fallback re-arms this slot's boundary through it: the hook
    /// admission armed belongs to the turn the message was admitted into, and that
    /// turn can end before the delivery writes — the prompt that replaces the
    /// steer then starts a turn of its own, and that turn is the boundary the slot
    /// has to end on.
    pub message_slot: Option<&'a MessageSlotRef<'a>>,
    /// The preset preamble this prompt carries in front of its own text, when the
    /// caller is a creation that has one.
    ///
    /// `None` for every other caller — a human's message, the app's own first
    /// prompt for a Design run, an agent message — and `Some(AGENT_PREAMBLE)` for
    /// the prompt an agent's creation sends to its child. It is a field of the
    /// request rather than something the send path looks up, so the ordering rule
    /// (standing instructions, then the profile's spawn prompt, then this, then
    /// the prompt) is composed in exactly one place and no session has to be
    /// searched for its preamble (`create-from-profile`).
    pub preset_preamble: Option<&'a str>,
    /// The resolved profile's spawn prompt, when the caller is a creation from a
    /// profile that carries one.
    ///
    /// A sibling of `preset_preamble` for the same reason: it is the caller's
    /// data, read when the creation was resolved, not something the send path
    /// looks up or re-reads later — so a prompt edited after the creation began
    /// cannot reach a child that has already started being born. `None` for
    /// every caller except that send, so profile text can only ever enter a
    /// prompt through the one composition point, in its fixed place between
    /// the standing instructions and the preamble.
    pub spawn_prompt: Option<&'a str>,
    /// Who authored this prompt and what part it plays in the target transcript.
    /// Required so every caller states both facts independently.
    pub author: UserMessageAuthor,
    pub message_kind: UserMessageKind,
}

/// What one delivery needs to re-key its brake slot (S4-10): the table, the
/// sender's key in it, and the slot.
pub(crate) struct MessageSlotRef<'a> {
    pub(crate) brakes: &'a Arc<Mutex<MessageBrakeTable>>,
    pub(crate) brake_key: &'a str,
    pub(crate) slot: u64,
    /// The turn the admission registered the boundary against (S4-14). The delivery
    /// compares it with the turn that is running when it writes, so a message whose
    /// admitted turn has been replaced is re-keyed onto the turn it actually enters.
    pub(crate) admitted_turn_id: u64,
}

/// A session's first prompt, composed in the one place (`create-from-profile`).
///
/// The order is fixed, and pinned by
/// `the_spawn_prompt_sits_between_the_standing_instructions_and_the_preamble`
/// and `standing_instructions_come_before_the_preset_preamble`: the human's
/// **standing instructions**, then the **profile's spawn prompt** where the
/// creation carries one, then the **preset preamble** where the caller has
/// one, then the prompt itself.
///
/// One glue point, on the shared send path every provider's writer sits behind.
/// That is the measured decision, not a preference: the daemon sends no system
/// prompt on any provider, and the preamble reaches the model today as the first
/// *user* message (`reports/remote-agents/recon-system-prompt-seams.md` §2 — the
/// glue at this same site, four writers, and ACP v1's `session/new` and
/// `session/prompt` carry no field for one). Composing here is what makes every
/// provider get the same text the same way, so none of them can be the silent
/// exception.
///
/// Everything empty means the prompt itself, **byte for byte**: a human who has
/// written no standing instructions, a profile with no spawn prompt, and a
/// caller with no preamble get exactly the bare prompt, with no separator and
/// no trailing newline to show for a feature they are not using.
pub(crate) fn compose_first_prompt(
    standing: &str,
    spawn: Option<&str>,
    preamble: Option<&str>,
    prompt: &str,
) -> String {
    [Some(standing), spawn, preamble, Some(prompt)]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The profile facts a `devboule_set_agent_profile` move delivers, resolved on
/// the caller's side at the moment of the move.
///
/// Plain data, so the registry never reads the profile store and the broker
/// never touches a session: the broker resolves the profile (§2 check 3) and
/// hands over what the move will ask the provider to apply. Resolved *inside*
/// the move, after the child check, so the refusals keep the spec's order
/// whatever the caller's convenience.
#[derive(Debug)]
pub(crate) struct ChildProfileFacts {
    pub(crate) profile_id: String,
    pub(crate) mode_id: String,
    pub(crate) model: String,
    pub(crate) thinking_option_id: Option<String>,
}

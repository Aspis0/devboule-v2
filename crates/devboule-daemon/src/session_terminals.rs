//! The registry side of the broker's five terminal tools: which terminals an
//! owner may reach inside one workspace, the visible screen of one of them,
//! and the three writes — open a terminal, type into one, kill one.
//!
//! The roster, the screen and the two writes that name a terminal all judge
//! an entry with [`reachable_terminal`]: the registry's own ownership door
//! ([`check_user_owner`], so a `Daemon` peer is scoped by origin, a `Client`
//! peer by the user that paired it, and a local call by user alone),
//! `kind == Terminal`, the caller's own workspace, and a process that is
//! still there — one rule, so an id outside it answers "No session with that
//! id." whichever of those it broke. The open names no terminal: it takes
//! its workspace from the caller's own row ([`Self::terminal_scope`]) and
//! counts what it may open against [`Self::reserve_terminal_slot`]. The
//! scope comes from session rows, never from a request field.

use super::*;

impl super::SessionRegistry {
    /// The workspace the calling session's own row names, which is the scope
    /// every terminal act is judged against. Read through the same door a
    /// target goes through, so a peer whose identity does not reach its own
    /// row never reaches a terminal either.
    pub(crate) fn terminal_scope(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
    ) -> Result<Option<String>, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let entry = peer_entry(&map, session_id, owner, conn_peer)?;
        Ok(entry.metadata().workspace_id.clone())
    }

    /// The running terminals of `owner` inside `workspace_id`.
    ///
    /// Only reachable entries count, so the roster never names a terminal the
    /// screen read would then refuse: a terminal whose process has gone keeps
    /// its journal row as history and is not an answer to "what can I read".
    pub(crate) fn terminals_in_workspace(
        &self,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
        workspace_id: &str,
    ) -> Result<Vec<Session>, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let mut terminals = map
            .values()
            .filter_map(|entry| reachable_terminal(entry, owner, conn_peer, workspace_id))
            .map(|(_, view)| view)
            .collect::<Vec<_>>();
        terminals.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(terminals)
    }

    /// The visible screen of one reachable terminal of `owner` inside
    /// `workspace_id`, windowed to `lines` rows from the bottom of what the
    /// terminal showed — blank rows below the last non-empty one dropped
    /// first — and that view's height for the caller to compare against.
    ///
    /// Every id outside that scope answers this one refusal: another owner's
    /// terminal, a terminal of another peer's origin, another workspace's
    /// terminal, an agent session, a terminal whose process has gone, and an
    /// id the daemon does not know. The sentence never says which of them the
    /// id named — the ownership door's own refusal is folded into it, so a
    /// caller learns nothing about an id it may not read.
    pub(crate) fn terminal_screen(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
        workspace_id: &str,
        lines: usize,
    ) -> Result<(Vec<String>, usize), WireError> {
        let runtime = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(session_id).ok_or_else(not_found)?;
            let (live, _) =
                reachable_terminal(entry, owner, conn_peer, workspace_id).ok_or_else(not_found)?;
            Arc::clone(&live.runtime)
        };
        // The cells were copied under the stream lock above; the rows are
        // formatted here, and only the rows the caller asked for.
        let screen = runtime.screen_snapshot().ok_or_else(not_found)?;
        Ok(screen.plain_rows_bottom(lines))
    }

    /// The one reachable terminal a write names, as the consent card shows
    /// it: title and working directory.
    ///
    /// Read through the gate the write itself takes, so a card can never
    /// name a target the write would then refuse.
    pub(crate) fn terminal_target(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
        workspace_id: &str,
    ) -> Result<Session, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let entry = map.get(session_id).ok_or_else(not_found)?;
        let (_, view) =
            reachable_terminal(entry, owner, conn_peer, workspace_id).ok_or_else(not_found)?;
        Ok(view)
    }

    /// Type `bytes` into one reachable terminal of `owner` inside
    /// `workspace_id`.
    ///
    /// The gate answers before the writer is even looked at, so a write can
    /// never reach a provider's stdin: that is the writer of an agent
    /// session, and an agent session is not a terminal. The registry lock is
    /// released before the PTY lock is taken (`session.rs`, LOCKING ORDER),
    /// so no lock is ever held across the I/O below.
    pub(crate) fn terminal_send_bytes(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
        workspace_id: &str,
        bytes: &[u8],
    ) -> Result<(), WireError> {
        let writer = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(session_id).ok_or_else(not_found)?;
            let (live, _) =
                reachable_terminal(entry, owner, conn_peer, workspace_id).ok_or_else(not_found)?;
            Arc::clone(&live.writer)
        };
        let mut writer = writer
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        writer.write_all(bytes).map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                // The failure carries the OS error, never the bytes: what was
                // typed stays out of refusals as it stays out of the audit.
                format!("Could not send input to the terminal: {error}"),
            )
        })?;
        writer.flush().map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not flush terminal input: {error}"),
            )
        })
    }

    /// Kill one reachable terminal of `owner` inside `workspace_id`: the
    /// gate, then the registry's own close path, which kills the process
    /// tree and removes the live entry while the journal row and the
    /// transcript stay.
    ///
    /// Answers whether a live session was removed, so the caller can hand
    /// back the live-session slot its create took (`session_started`).
    pub(crate) fn kill_terminal(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
        workspace_id: &str,
    ) -> Result<bool, WireError> {
        {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(session_id).ok_or_else(not_found)?;
            // The gate before the close, not inside it: `close` itself checks
            // ownership but knows nothing about kinds, and without this an
            // agent session's id would end a provider session.
            reachable_terminal(entry, owner, conn_peer, workspace_id).ok_or_else(not_found)?;
        }
        #[cfg(test)]
        self.fire_kill_after_gate_hook();
        self.close(session_id, owner, conn_peer)
    }

    /// Take one slot of the live-terminal cap for `creator`, before anything
    /// is spent asking a person for it.
    ///
    /// Live terminals and the slots other creates are already holding are
    /// counted under one lock, taken *before* the registry lock and never
    /// after it, so two creates racing at the cap cannot both read room and
    /// both spawn. The slot is released by the reservation's own `Drop`, so
    /// every exit from the create — a refusal, a denied card, a failed spawn,
    /// the answer — gives it back exactly once without a release line per
    /// path.
    ///
    /// Counted from the `created_by` link the create stamps, under the same
    /// ownership door every terminal act takes: a `Daemon` peer counts the
    /// terminals of its own origin, a `Client` peer those of the user who
    /// paired it, a local call those of its user.
    pub(crate) fn reserve_terminal_slot(
        &self,
        creator: &str,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
    ) -> Result<TerminalSlotReservation<'_>, WireError> {
        let unavailable = || internal("Session state is unavailable.");
        let mut slots = self.terminal_slots.lock().map_err(|_| unavailable())?;
        let held = slots.get(creator).copied().unwrap_or(0);
        let open = {
            let map = self.inner.lock().map_err(|_| unavailable())?;
            let mut open = 0;
            for entry in map.values() {
                let Some(session) = entry.as_peer_visible() else {
                    continue;
                };
                if session.metadata.kind != SessionKind::Terminal
                    || !is_child_of(session.metadata.created_by.as_deref(), creator)
                    || check_user_owner(entry, owner, conn_peer).is_err()
                {
                    continue;
                }
                if live_session_view(session).state.is_live() {
                    open += 1;
                }
            }
            open
        };
        if open + held >= MAX_LIVE_TERMINALS_PER_CREATOR {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                TERMINAL_CAP_REFUSAL,
            ));
        }
        slots.insert(creator.to_string(), held + 1);
        Ok(TerminalSlotReservation {
            slots: &self.terminal_slots,
            creator: creator.to_string(),
        })
    }

    /// Open a terminal in `workspace_id` for the session that asked: the
    /// wire's own create road, with `created_by` stamped — the link the cap
    /// above counts and the roster's creator column reads.
    ///
    /// `env_provider` is `None` on purpose: the agent-provider override is
    /// not a terminal's business, and the terminal family resolves its own
    /// shell.
    pub(crate) fn create_terminal_for(
        &self,
        state: &Arc<ServerState>,
        owner: &OwnerId,
        workspace_id: Option<String>,
        display_name: Option<String>,
        conn_peer: &Option<ConnPeer>,
        creator: &str,
    ) -> Result<Session, WireError> {
        let meta = SessionCreateMeta {
            display_name,
            created_by: Some(creator.to_string()),
            ..SessionCreateMeta::default()
        };
        self.create_with_provider_env(
            state,
            owner,
            workspace_id,
            SessionKind::Terminal,
            None,
            crate::profile_delivery::ProfileDelivery::for_request(None),
            None,
            conn_peer,
            None,
            &meta,
            None,
        )
    }
}

/// The one refusal of [`SessionRegistry::reserve_terminal_slot`], in the
/// shape the other caps refuse in and actionable for whoever hit it.
const TERMINAL_CAP_REFUSAL: &str = "too many live terminals; close one first";

/// One terminal-open slot, held from before the consent card until the create
/// has answered. Released by `Drop`, so every exit path gives it back — the
/// card's denial, the shutdown guard, a spawn that failed, the reply.
pub(crate) struct TerminalSlotReservation<'a> {
    slots: &'a Mutex<HashMap<String, usize>>,
    creator: String,
}

impl Drop for TerminalSlotReservation<'_> {
    fn drop(&mut self) {
        // Poisoned or not, the count goes back: swallowing the poison here
        // would leak the reservation until the process restarts.
        let mut slots = match self.slots.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        match slots.get(&self.creator).copied() {
            Some(held) if held <= 1 => {
                slots.remove(&self.creator);
            }
            Some(held) => {
                slots.insert(self.creator.clone(), held - 1);
            }
            None => {}
        }
    }
}

/// One registry entry as every tool that names a terminal judges it — the
/// roster's entries, the screen, the keys write and the kill write, never
/// the open, which names no target. Reachable only when [`check_user_owner`]
/// opens it — the check every id-addressed session act goes through — when
/// it is a live entry of `kind == Terminal` inside `workspace_id`, and when
/// its process is still there.
///
/// The second value is the entry's up-to-date view, state recomputed from the
/// runtime rather than read from possibly stale metadata, so a body never
/// judges or answers a state the process has already left. Configuring
/// entries, transcripts, agent sessions, another workspace's terminal,
/// another owner's terminal and an exited terminal are all `None` — one
/// answer for all of them, so a caller cannot tell which an id named.
fn reachable_terminal<'a>(
    entry: &'a RegistryEntry,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
    workspace_id: &str,
) -> Option<(&'a PtySession, Session)> {
    if check_user_owner(entry, owner, conn_peer).is_err() {
        return None;
    }
    let live = entry.as_peer_visible()?;
    if live.metadata.kind != SessionKind::Terminal
        || live.metadata.workspace_id.as_deref() != Some(workspace_id)
    {
        return None;
    }
    let view = live_session_view(live);
    view.state.is_live().then_some((live, view))
}

//! The terminal reads behind the broker's two terminal tools: which terminals
//! an owner may list inside one workspace, and the visible screen of one of
//! them.
//!
//! Every read goes through the registry's own ownership door —
//! [`check_user_owner`], the check every id-addressed session read uses — so
//! a `Daemon` peer is scoped by origin, a `Client` peer by the user that
//! paired it, and a local call by user alone. Only `kind == Terminal` inside
//! the caller's own workspace counts, and the scope comes from session rows,
//! never from a request field.

use super::*;

impl super::SessionRegistry {
    /// The workspace the calling session's own row names, which is the scope
    /// both reads are judged against. Read through the same door a target
    /// goes through, so a peer whose identity does not reach its own row
    /// never reaches a read either.
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
    /// Only live entries count, so the roster never names a terminal the
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
        let mut terminals = Vec::new();
        for entry in map.values() {
            if check_user_owner(entry, owner, conn_peer).is_err() {
                continue;
            }
            // Configuring entries and transcript-only entries are not live
            // terminals for a reader, whatever their stored metadata says.
            let Some(live) = entry.as_peer_visible() else {
                continue;
            };
            if live.metadata.kind != SessionKind::Terminal
                || live.metadata.workspace_id.as_deref() != Some(workspace_id)
            {
                continue;
            }
            let session = live_session_view(live);
            if session.state.is_live() {
                terminals.push(session);
            }
        }
        terminals.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(terminals)
    }

    /// The visible screen of one running terminal of `owner` inside
    /// `workspace_id`, windowed to `lines` rows from the bottom of the grid,
    /// and the grid's full height for the caller to compare against.
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
            if check_user_owner(entry, owner, conn_peer).is_err() {
                return Err(not_found());
            }
            // A configuring entry or a transcript has no screen to read.
            let Some(live) = entry.as_peer_visible() else {
                return Err(not_found());
            };
            if live.metadata.kind != SessionKind::Terminal
                || live.metadata.workspace_id.as_deref() != Some(workspace_id)
            {
                return Err(not_found());
            }
            if !live_session_view(live).state.is_live() {
                return Err(not_found());
            }
            Arc::clone(&live.runtime)
        };
        // The cells were copied under the stream lock above; the rows are
        // formatted here, and only the rows the caller asked for.
        let screen = runtime.screen_snapshot().ok_or_else(not_found)?;
        let total = usize::from(screen.rows);
        Ok((screen.plain_rows_from(total.saturating_sub(lines)), total))
    }
}

//! The terminal reads behind the broker's two terminal tools: which terminals
//! an owner may list inside one workspace, and the visible screen of one of
//! them.
//!
//! Both reads are scoped twice — the caller's own user, and the caller's own
//! workspace — and only `kind == Terminal` counts, so an agent session is
//! never part of a terminal answer. The scope is read from session rows and
//! never from a request field.

use super::*;

impl super::SessionRegistry {
    /// The terminals of `owner` inside `workspace_id`, live and exited alike.
    ///
    /// [`Self::list`] already merges the live map with the journal rows for
    /// the same user, so an exited terminal is still listed — with `live`
    /// false on the wire — while the kind and workspace filters take agent
    /// sessions and other projects' terminals out.
    pub(crate) fn terminals_in_workspace(
        &self,
        owner: &OwnerId,
        workspace_id: &str,
    ) -> Result<Vec<Session>, WireError> {
        let sessions = self.list(owner)?;
        Ok(sessions
            .into_iter()
            .filter(|session| {
                session.kind == SessionKind::Terminal
                    && session.workspace_id.as_deref() == Some(workspace_id)
            })
            .collect())
    }

    /// The visible screen of one terminal of `owner` inside `workspace_id`,
    /// as plain rows of text.
    ///
    /// Every id outside that scope answers this one refusal: another owner's
    /// terminal, another workspace's terminal, an agent session, a terminal
    /// whose process is gone, and an id the daemon does not know. The
    /// sentence never says which of them the id named — a dead terminal is
    /// refused rather than answered from a screen its process no longer
    /// drives.
    pub(crate) fn terminal_screen(
        &self,
        session_id: &str,
        owner: &OwnerId,
        workspace_id: &str,
    ) -> Result<Vec<String>, WireError> {
        let runtime = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(session_id).ok_or_else(not_found)?;
            if entry.owner().user != owner.user || entry.is_configuring() {
                return Err(not_found());
            }
            let view = entry.to_session();
            if view.kind != SessionKind::Terminal
                || view.workspace_id.as_deref() != Some(workspace_id)
                || !view.state.is_live()
            {
                return Err(not_found());
            }
            entry.runtime()
        };
        let screen = runtime.screen_snapshot().ok_or_else(not_found)?;
        Ok(screen.plain_rows())
    }
}

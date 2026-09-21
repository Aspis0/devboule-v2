//! The resume road's named phases, split out of `SessionRegistry::resume` (a
//! rewrite, not a move: the characterisation tests in
//! `session_resume_tests.rs` are its proof). The method stays in `session.rs`
//! as the thin sequence — locate the row, stage the command, evict the
//! previous instance, gate and take the slot, read the lineage, register,
//! open the generation, readmit the child, spawn — and the four
//! `session_finished` call sites stay with it, because their identity is the
//! side effect and not a predicate.

use super::*;

/// The sentence a resume answers with when the directory the session worked in
/// is gone. Spelled once, because the same words have to reach a human two
/// ways: as the refusal a session with nothing to recover gets, and inside the
/// notice a recovered session carries in its own transcript.
pub(super) fn session_folder_gone(path: &str) -> WireError {
    WireError::new(
        ErrorCode::WorkspaceUnavailable,
        format!(
            "the folder this session worked in no longer exists: {}",
            crate::workspace::display_path(path)
        ),
    )
}

/// The same sentence for the other refusal a recovery follows: the far side
/// answered that it does not have this session. The daemon's fact comes first
/// and the provider's own words are quoted after it, the shape
/// [`session_folder_gone`] established — the notice a recovered session shows
/// is where a human reads *why* the session they clicked could not be
/// reopened, and the folder sentence would be a lie here.
///
/// The code is the internal disown sentinel and never reaches the caller: the
/// refused handle keeps its original error, remapped to the wire code its
/// family has always answered, so this value travels only as far as the notice
/// and the recovered prompt.
pub(super) fn provider_refused_session(message: &str) -> WireError {
    WireError::new(
        ErrorCode::SessionNotFound,
        format!("the provider no longer has this session: {message}"),
    )
}

/// A previous-run transcript is replaced without a teardown — it holds no
/// process — but the pin it took when a client read it must go with it, or
/// the row is never reclaimable again and nothing fails.
fn resume_evict_stale_transcript(
    session: &TranscriptSession,
    journal: &Journal,
    session_id: &str,
    conn: &ConnHandle,
) {
    session.runtime.detach_if_conn(conn.id);
    session.runtime.notify_generation_replaced(conn.id);
    journal.unpin(session_id);
}

/// The order test's one-shot rendezvous: the phase test arms it for one
/// session id and that session's detached thread fires it between the two
/// writes. Keyed by id so a concurrent resume of another session cannot take
/// it; unarmed, the road pays one lock and nothing else.
#[cfg(test)]
static BETWEEN_WRITES_HOOK: Mutex<Option<(String, BetweenWritesHook)>> = Mutex::new(None);

#[cfg(test)]
type BetweenWritesHook = Box<dyn FnOnce() + Send>;

#[cfg(test)]
pub(super) fn arm_between_writes_hook(session_id: &str, hook: impl FnOnce() + Send + 'static) {
    let armed = (session_id.to_string(), Box::new(hook) as BetweenWritesHook);
    *BETWEEN_WRITES_HOOK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = Some(armed);
}

#[cfg(test)]
fn fire_between_writes_hook(session_id: &str) {
    let hook = {
        let mut armed = BETWEEN_WRITES_HOOK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let armed_for_this_session = armed
            .as_ref()
            .is_some_and(|(armed_id, _)| armed_id == session_id);
        if armed_for_this_session {
            armed.take().map(|(_, hook)| hook)
        } else {
            None
        }
    };
    if let Some(hook) = hook {
        hook();
    }
}

/// The failed respawn's end marker, on a throwaway thread: the blocking send
/// is an unbounded busy-loop with no timeout, so it must not freeze the
/// dispatch thread, and the disown fallback rides it FIRST — this thread
/// exists for the saturated-queue case, where delaying that write is exactly
/// the harm. The order is load-bearing and pinned by
/// `the_disown_mark_lands_before_the_end_marker`: the half-state it avoids,
/// `{status = ended, disowned = NULL}`, still reads `resumable: true`. Both
/// writes are best effort and the thread is detached and unjoined: a daemon
/// that exits in this window can lose the fallback silently; the
/// dispatch-thread write it backs up is the ordered one.
pub(super) fn resume_end_generation_detached(
    journal: &Arc<Journal>,
    session_id: &str,
    generation: u64,
    peer_disowned: bool,
    attempted_handle: String,
) {
    let journal = Arc::clone(journal);
    let id = session_id.to_string();
    let _ = std::thread::Builder::new()
        .name("journal-end-marker".into())
        .spawn(move || {
            if peer_disowned {
                let _ = journal.mark_peer_session_disowned_blocking(&id, &attempted_handle);
            }
            #[cfg(test)]
            fire_between_writes_hook(&id);
            let _ = journal.mark_ended_blocking(&id, generation, None);
        });
}

impl SessionRegistry {
    /// The row the resume continues, out of the journal the same read proves
    /// is reachable. The provider file is refreshed at this boundary for the
    /// same reason the create road's is: the row a session was created under
    /// must still resolve here.
    pub(super) fn resume_locate_record(
        &self,
        session_id: &str,
    ) -> Result<(Arc<Journal>, SessionRecord), WireError> {
        crate::user_providers::refresh_user_rows(self.runtime_dir());
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        let record = journal
            .list()?
            .into_iter()
            .find(|record| record.id == session_id)
            .ok_or_else(not_found)?;
        Ok((Arc::clone(journal), record))
    }

    /// The command the respawned provider is launched with, and the
    /// generation it opens on the row.
    ///
    /// The persisted provider is the original explicit choice: a persisted
    /// npx wrapper is allowed through this named path because its original
    /// create already supplied consent. It is resolved through the record's
    /// own family, not the ACP road — an ACP row takes the named catalog row
    /// exactly as before, a Claude row its fixed stream-json command.
    pub(super) fn resume_stage_command(
        &self,
        record: &SessionRecord,
        provider: &str,
    ) -> Result<(PtyCommand, u64), WireError> {
        let family = provider::catalog_registry().provider_for_kind(&record.kind);
        let mut command = family.resolve_command(&self.paths, Some(provider))?;
        match record.workspace_id.as_deref() {
            // The workspace road, unchanged: the store resolves the id and
            // answers `WorkspaceUnavailable` when its folder is gone, in the
            // words the app already renders for that case.
            Some(workspace_id) => {
                self.apply_workspace_cwd(Some(workspace_id), &mut command)?;
            }
            // A session with no workspace has no other record of where it
            // worked than the directory its birth wrote down. When that
            // directory is gone the resume is refused **here, in words**, and
            // nothing is spawned: the provider's answer to a `cwd` that does
            // not exist is `Invalid params` — the measured case, 42 ms after
            // the request — and a child launched to deliver it is a process,
            // an npx wrapper and a handshake spent on a question the daemon
            // could answer itself.
            //
            // A row with no recorded directory — every row that predates v15,
            // and every row nothing ever launched — keeps the command's own
            // default, which is exactly what this road did before the column
            // existed: the resume is not made stricter by a fact nobody
            // recorded.
            None => {
                if let Some(cwd) = record.cwd.as_deref() {
                    let path = PathBuf::from(cwd);
                    if !path.is_dir() {
                        return Err(session_folder_gone(cwd));
                    }
                    command.cwd = path;
                }
            }
        }
        Ok((command, record.generation.saturating_add(1)))
    }

    /// Evict the previous instance of this id, in the single `inner` hold that
    /// also answers whether it held a live slot — the slot a resumed session
    /// inherits instead of taking a new one.
    ///
    /// A previous-run transcript is replaced, and a stopped live entry is
    /// replaced only after it has been observed dead; resuming a still live
    /// process would create two writers for one session id.
    pub(super) fn resume_evict_previous(
        &self,
        journal: &Arc<Journal>,
        session_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<bool, WireError> {
        let (old_entry, had_live_slot) = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            if let Some(entry) = map.get(session_id) {
                check_user_owner(entry, owner, &conn.conn_peer)?;
                // *"Is the child that holds this entry still running?"* —
                // asked over `as_child_process`, because a `Configuring`
                // entry is a running child the same way a `Live` one is.
                // Over the peer-visibility accessor the refusal silently
                // stopped covering the delivery window, and a resume there
                // replaced a running child out from under its in-flight
                // create (the re-audit's P2-1).
                if entry
                    .as_child_process()
                    .is_some_and(|session| !session.runtime.process_exited())
                {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        "This session cannot be resumed while its process is running.",
                    ));
                }
            }
            let old_entry = map.remove(session_id);
            let had_live_slot = matches!(
                old_entry,
                Some(RegistryEntry::Live(_)) | Some(RegistryEntry::Configuring(_))
            );
            (old_entry, had_live_slot)
        };
        if let Some(old_entry) = old_entry {
            match old_entry {
                RegistryEntry::Live(session) | RegistryEntry::Configuring(session) => {
                    // A resume replaces a live entry: the process that held it
                    // is gone, so this is a child's end like any other (`S5`
                    // decisions 7 and 8, audit S5-01) — reported once and its
                    // slot released, whether the resume then succeeds or fails.
                    // A resumed session is not a creation, so the session that
                    // comes back has no row to release later.
                    self.child_ended_with(
                        session_id,
                        Some(&session.metadata),
                        Some(&session.runtime),
                        Some(owner),
                    );
                    session.runtime.detach_if_conn(conn.id);
                    session.runtime.notify_generation_replaced(conn.id);
                    teardown_session_for_resume(*session);
                }
                RegistryEntry::Transcript(session) => {
                    resume_evict_stale_transcript(&session, journal, session_id, conn);
                }
            }
        }
        Ok(had_live_slot)
    }

    /// The registered entry the spawn left behind. One `inner` hold, and
    /// nothing inside it that takes the same lock: `inner` is not reentrant.
    pub(super) fn resume_read_registered(&self, session_id: &str) -> Result<Session, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        map.get(session_id)
            .map(RegistryEntry::to_session)
            .ok_or_else(|| internal("resumed session was not registered"))
    }
}

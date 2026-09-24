//! The create road's named phases, split out of
//! `SessionRegistry::create_with_provider_env` (a rewrite, not a move: the
//! characterisation tests in `session_create_tests.rs` are its proof). The
//! method itself stays in `session.rs` and remains the thin sequence: resolve
//! inputs, stamp the birth, build the record, then the durable boundary —
//! journal row, env injection, MCP registration, pending-child note, spawn —
//! with only the spawn-failure arm handed to `fail_spawn`.

use super::*;

/// What the create road resolved before anything is stamped or written.
pub(super) struct ResolvedCreation {
    pub(super) id: String,
    pub(super) kind: SessionKind,
    pub(super) command: PtyCommand,
    pub(super) session_provider: Option<String>,
}

impl super::SessionRegistry {
    /// The cwd/id/provider/command resolution. The only lock it reaches is
    /// `workspace_paths` inside `workspace_cwd`, and the workspace lookup
    /// deliberately runs before the `meta.cwd` override: an unknown workspace
    /// refuses even when the create carries its own directory.
    // The caller's creation request travels as arguments, the same tail the
    // entry point itself is allowed to take.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn resolve_creation_inputs(
        &self,
        owner: &OwnerId,
        workspace_id: Option<&str>,
        kind: SessionKind,
        provider: Option<String>,
        env_provider: Option<&str>,
        command: Option<PtyCommand>,
        meta: &SessionCreateMeta,
    ) -> Result<ResolvedCreation, WireError> {
        let workspace_cwd = workspace_id
            .map(|workspace_id| self.workspace_cwd(workspace_id))
            .transpose()?;
        // A created child may start in a subdirectory of the creator's
        // workspace. It was resolved and confined on the way in
        // (`confined_child_cwd`), so a path that reaches here is already inside
        // the workspace, canonical, and an existing directory.
        let workspace_cwd = match meta.cwd.clone() {
            Some(cwd) => Some(cwd),
            None => workspace_cwd,
        };
        let id = match meta.session_id.clone() {
            Some(id) => id,
            None => compose_session_id(&owner.session_token(), &mint_session_unique())
                .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?,
        };
        let (kind, provider, provenance) =
            Self::resolve_session_provider(kind, provider, env_provider);
        let mut command = match command {
            Some(command) => command,
            None => {
                // The command road is the registry's to resolve: the kind
                // names its family, the family resolves its own command —
                // the ACP family's named road consults the catalog, the
                // native families run their fixed roads, the terminal road
                // resolves the shell.
                let family = provider::catalog_registry().provider_for_kind(&kind);
                // The npx consent gate is catalog policy (design §3.3.5) and
                // stays keyed on the one family whose named road can resolve
                // a catalog wrapper, not on the kind.
                if let Some(id) = provider.as_deref() {
                    if family.resolves_named_from_catalog() {
                        Self::reject_env_npx_wrapper(id, provenance, &self.paths)?;
                    }
                }
                family.resolve_command(&self.paths, provider.as_deref())?
            }
        };
        if let Some(cwd) = workspace_cwd {
            command.cwd = cwd;
        }
        let session_provider = provider::catalog_registry()
            .provider_for_kind(&kind)
            .stamp_session_provider(provider.clone(), command.provider_id.clone());
        Ok(ResolvedCreation {
            id,
            kind,
            command,
            session_provider,
        })
    }

    /// Park the child-end marker for an agent's child, exactly when the
    /// caller said the creation is still pending (audit-2 §2): the marker is
    /// noted after journalling and before the spawn, and the reservation is
    /// consumed once, by the commit.
    pub(super) fn note_pending_child_if_creation_pending(
        &self,
        id: &str,
        meta: &SessionCreateMeta,
    ) {
        if meta.creation_pending {
            self.note_pending_child(
                id,
                meta.reservation
                    .expect("an agent child holds a reservation"),
            );
        }
    }

    /// The spawn-failure arm, in its load-bearing order: release the pending
    /// marker, end the born row asynchronously (no blocking journal wait on
    /// the dispatch thread), then record provider health only behind the
    /// `spawn_failure_is_provider_health` gate — a profile's InvalidRequest
    /// refusal must not mark a healthy provider unhealthy (the R2a audit's
    /// F6).
    pub(super) fn fail_spawn(
        &self,
        state: &Arc<ServerState>,
        session_id: &str,
        record_generation: u64,
        provider_id: Option<&str>,
        error: WireError,
    ) -> WireError {
        // The token rollback clears what the reservation noted: the
        // creation never became a child, so nothing is owed to anyone
        // (audit-2 §2).
        self.clear_pending_child(session_id);
        if let Some(journal) = &self.journal {
            spawn_async_end_marker(journal, session_id, record_generation);
        }
        if let Some(provider_id) = provider_id {
            if spawn_failure_is_provider_health(&error) {
                state.record_provider_health(provider_id, Err(&error));
            }
        }
        error
    }
}

/// The origin and title stamps, pure: a created child inherits its creator's
/// stored origin and never re-derives one from the connection this thread
/// happens to hold — the MCP connection of a peer's session is a loopback
/// socket, and reading *it* would label a peer's child as this machine's own
/// (S5 checklist). S9: agent-ness is one protocol predicate, not a kind list.
pub(super) fn birth_stamps(
    meta: &SessionCreateMeta,
    conn_peer: &Option<ConnPeer>,
    kind: &SessionKind,
) -> (SessionOrigin, String) {
    let origin = meta
        .origin
        .clone()
        .unwrap_or_else(|| session_origin_for(conn_peer));
    let title = match meta.display_name.clone() {
        Some(name) => name,
        None => {
            if kind.is_agent() {
                "Agent".to_string()
            } else {
                "Terminal".to_string()
            }
        }
    };
    (origin, title)
}

/// The birth record and the wire metadata, pure and lock-free, from one
/// set of stamps so the journal row and the metadata carry the same facts —
/// including the same instant (`record.created_at_ms`).
pub(super) fn build_birth_record(
    resolved: &ResolvedCreation,
    owner: &OwnerId,
    workspace_id: Option<String>,
    delivery: &crate::profile_delivery::ProfileDelivery,
    meta: &SessionCreateMeta,
    origin: SessionOrigin,
    title: String,
) -> (SessionRecord, Session, u64) {
    let id = resolved.id.as_str();
    let kind = &resolved.kind;
    let session_provider = resolved.session_provider.clone();
    let mut record = new_session_record(
        id.to_string(),
        owner.user.clone(),
        workspace_id.clone(),
        kind.clone(),
        title,
    );
    record.provider = session_provider.clone();
    // The directory this session is about to be launched in, recorded at
    // birth: the one fact that lets a later resume check where it worked
    // before it spawns anything. Raw, not the display form — this value is
    // handed back to a process one day (the brief's "the cwd the daemon really
    // used"), and only `to_session` renders it.
    record.cwd = Some(resolved.command.cwd.to_string_lossy().into_owned());
    // The name a human reads and the session that asked for this one are
    // the row's, not just the wire metadata's (audit S5-12): an app that
    // attaches to this daemon after a restart lists its sessions from the
    // journal, and a child that came back without its name and its parent
    // would be a different session than the one that was created.
    record.display_name = meta.display_name.clone();
    record.created_by = meta.created_by.clone();
    // The creation-from-profile facts, written once, here, and never
    // re-derived from the store afterwards (v11). A create that resolved no
    // profile — the human's provider picker, a terminal — leaves them at
    // their defaults, and a create that did leaves the daemon's own record
    // of it: the profile's **stable id** (a rename later cannot make this
    // child misreport what it was started from), the labels the creation
    // stamped, and the context this session belongs to.
    record.profile_id = meta.profile_id.clone();
    // The overlay the creation resolved at birth, written here with the
    // other birth facts and never re-resolved: the profile it came from
    // may be edited or deleted afterwards, and the child's powers were
    // decided when it was born.
    record.overlay = Some(meta.overlay.clone());
    // The child's own depth, same rule: the cap must survive a restart,
    // and re-deriving it by walking `created_by` would trust a chain
    // the retention sweep may have cut.
    record.depth = Some(meta.depth);
    // The marker, derived here from the **delivered** mode (R2b): this is
    // the one place the kind and the delivery the child is started on meet
    // the row, so the marker is the delivery's own judgement — a profile's
    // feature tick is not an input, and a create that resolved no profile
    // is judged by its family's own default. ACP vocabularies are the
    // agent's own prose, so they answer `unknown` unless the daemon's
    // broker itself answers the delivered id.
    let unattended_state =
        crate::peer_policy::unattended_mode(kind.clone(), delivery.mode_id.as_deref());
    record.unattended_state = unattended_state;
    record.labels = meta.labels.clone();
    // Its own id, unless its creator's context came in with the creation:
    // that inheritance is the whole rule, and it is applied once, here, so
    // every reader — the roster, the journal, the A2A answer — sees one
    // value.
    let context_id = meta.context_id.clone().unwrap_or_else(|| id.to_string());
    record.context_id = Some(context_id.clone());
    record.status = PersistStatus::Live;
    // The origin is a property of the create, not of the spawn: it is
    // recorded before the row is journaled, so a create that dies during
    // spawn still reads back as the device that asked for it.
    record.origin = origin.clone();
    let record_generation = record.generation;
    let metadata = Session {
        id: id.to_string(),
        workspace_id,
        cwd: Some(crate::workspace::plain_path(
            &resolved.command.cwd.to_string_lossy(),
        )),
        kind: kind.clone(),
        title: record.title.clone(),
        provider: session_provider.clone(),
        peer_session_id: None,
        state: SessionState::Live { generation: 1 },
        elapsed_ms: Some(0),
        created_at_ms: record.created_at_ms,
        origin,
        display_name: meta.display_name.clone(),
        created_by: meta.created_by.clone(),
        profile_id: meta.profile_id.clone(),
        context_id: Some(context_id),
        unattended: unattended_state,
        labels: meta.labels.clone(),
        // Born live: the process exists, so resume is refused. Views
        // recompute on every serve.
        resumable: false,
    };
    (record, metadata, record_generation)
}

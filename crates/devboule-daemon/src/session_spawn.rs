//! Process birth and death: the liveness sweeper, the spawn roads, the PTY
//! reader and its coalescer, and the teardown order.
//!
//! Split out of `session.rs` without a rewrite: every line below this header
//! is byte-identical to its text there, apart from the `pub(super)` markers a
//! caller in the parent module (or in `provider.rs`, a sibling) reaches in for
//! — seven functions, and the three fields of `ResumedSessionContext` that the
//! parent still builds.

use super::*;

pub(super) fn spawn_os_liveness_sweeper(registry: &SessionRegistry) {
    let inner = Arc::downgrade(&registry.inner);
    let sink = Arc::downgrade(&registry.transition_sink);
    if let Err(error) = std::thread::Builder::new()
        .name("session-os-liveness".to_string())
        .spawn(move || loop {
            std::thread::sleep(SESSION_OS_SWEEP_INTERVAL);
            let Some(inner) = inner.upgrade() else {
                return;
            };
            let Some(sink) = sink.upgrade() else {
                return;
            };
            sweep_os_liveness(&inner, &sink);
        })
    {
        eprintln!("could not start OS liveness sweeper: {error}");
    }
}

fn sweep_os_liveness(
    inner: &Mutex<HashMap<String, RegistryEntry>>,
    sink: &Mutex<Option<TransitionSink>>,
) {
    let Ok(map) = inner.lock() else {
        return;
    };
    let work: Vec<(Arc<SessionRuntime>, OwnerId)> = map
        .values()
        // Peer visibility, deliberately: a windowed child's death is the
        // delivery's own refusal to observe (the awaited rpc times out and
        // the close tears it down), and the sweep's transitions must not
        // fire for a session no roster lists.
        .filter_map(|entry| {
            let session = entry.as_peer_visible()?;
            Some((Arc::clone(&session.runtime), session.owner.clone()))
        })
        .collect();
    drop(map);
    for (runtime, owner) in work {
        let newly_dead = runtime.observe_os_liveness();
        if newly_dead {
            runtime.fire_os_death();
        }
        let notify = if newly_dead || runtime.process_exited() {
            runtime.should_publish_exit_transition()
        } else {
            runtime.mark_silent_if_due(Instant::now()).is_some() && runtime.transition_ready()
        };
        if !notify {
            continue;
        }
        let callback = sink.lock().ok().and_then(|guard| guard.clone());
        if let Some(callback) = callback {
            callback(owner);
        }
    }
}

/// Whether a failed spawn says anything about the **provider's** health.
///
/// The clients refuse, before and around the spawn, every value the profile
/// alone decides — an unknown model, mode or thinking option, a catalogue
/// that publishes nothing, an `autoAccept` contradiction, an agent refusing
/// the delivered switch — and every one of those refusals is
/// `ErrorCode::InvalidRequest` by convention; nothing else on a spawn path
/// raises that code (a provider-side failure is `Io`/`Internal`, including
/// Pi's extension not activating). A profile mistake is the human's to fix
/// in the profile: recording it against the provider degrades the Settings
/// health line for a correctly installed provider (the R2a audit's F6).
pub(super) fn spawn_failure_is_provider_health(error: &WireError) -> bool {
    error.code != ErrorCode::InvalidRequest
}

pub fn spawn_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    owner: OwnerId,
    command: PtyCommand,
    mut mcp_session: Option<McpSessionGuard>,
    delivery: crate::profile_delivery::ProfileDelivery,
) -> Result<(), WireError> {
    // The delivery travels as the one typed value: each family's `spawn`
    // validates what it can refuse and applies what it owns. The dispatch is
    // the registry's — no arm matches on the kind or names a family — and
    // each family's workspace error mapping happens inside its own `spawn`,
    // which is why none is applied here.
    let workspace_id = metadata.workspace_id.clone();
    let spawned = provider::catalog_registry()
        .provider_for_kind(&metadata.kind)
        .spawn(
            state,
            command,
            state.mcp.launch_config(&metadata.id),
            delivery.clone(),
            workspace_id.as_deref(),
        )?;
    start_spawned_session(
        state,
        registry,
        metadata,
        owner,
        None,
        delivery.mode_id,
        spawned,
        mcp_session.take(),
    )
}

pub(crate) struct ResumedSessionContext {
    pub(super) peer_session_id: String,
    pub(super) generation: u64,
    pub(super) mcp_session: Option<McpSessionGuard>,
}

pub fn spawn_resumed_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    owner: OwnerId,
    command: PtyCommand,
    context: ResumedSessionContext,
) -> Result<(), WireError> {
    let mcp = state.mcp.launch_config(&metadata.id);
    // The family's own respawn: ACP reloads by session/load, Claude by
    // `--resume`, Codex by `thread/resume`. Anything the gate admitted
    // implements this; the refused families never reach here.
    let family = provider::catalog_registry().provider_for_kind(&metadata.kind);
    let spawned = family.spawn_resuming(state, command, context.peer_session_id, mcp)?;
    start_spawned_session(
        state,
        registry,
        metadata,
        owner,
        Some(context.generation),
        None,
        spawned,
        context.mcp_session,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn start_spawned_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    owner: OwnerId,
    generation: Option<u64>,
    requested_mode: Option<String>,
    spawned: SpawnedSession,
    mcp_session: Option<McpSessionGuard>,
) -> Result<(), WireError> {
    let SpawnedSession {
        process_job,
        master,
        killer,
        switcher,
        child,
        writer,
        image_sink,
        static_image_sink,
        out_of_band,
        reader,
        reader_dispatch,
        stderr,
        permission_broker,
        os_handle,
        peer_session_id,
        agent_version,
        pending_delivery,
        pending_codex_verify,
    } = spawned;
    if let (Some(provider_id), Some(version)) = (&metadata.provider, agent_version.as_deref()) {
        state.record_provider_version(provider_id, version);
    }
    let runtime = if metadata.kind.is_agent() {
        SessionRuntime::for_acp(
            metadata.id.clone(),
            registry.journal.clone(),
            permission_broker.expect("agent sessions have a permission broker"),
        )
    } else {
        Arc::new(SessionRuntime::with_journal(
            metadata.id.clone(),
            registry.journal.clone(),
        ))
    };
    if metadata.kind.is_agent() {
        runtime.set_agent_kind(metadata.kind.clone());
    }
    // The origin the create wrote travels with the metadata; installing it on
    // the runtime is what lets the permission broker stamp a card and the peer
    // gate answer `prompt_skipping` without a registry lookup.
    runtime.set_origin(metadata.origin.clone());
    // S9: hosting is one predicate; waiting is the narrower rule (S8: never
    // pi/Codex — the twin tests pin it). Binding is registration-fact-gated
    // inside `bind_runtime` itself (a lookup that no-ops without a row), so it
    // runs unconditionally: identical for every road but a minted carrier. The
    // guard is strict exactly where MCP gates the send path (a lost bearer
    // there must fail loudly, never leak); elsewhere the create road's
    // `Option` flows through untouched (tests and unregistered spawns).
    if crate::mcp_broker::mcp_gates_first_prompt(&metadata.kind) {
        runtime.require_mcp();
    }
    state.mcp.bind_runtime(&metadata.id, &runtime);
    let mcp_session = if crate::mcp_broker::mcp_gates_first_prompt(&metadata.kind) {
        Some(mcp_session.ok_or_else(|| {
            internal("MCP session registration was lost before provider startup.")
        })?)
    } else {
        mcp_session
    };
    if let Some(peer_session_id) = peer_session_id {
        runtime.set_peer_session_id(peer_session_id);
    }
    if let Some(generation) = generation {
        runtime.set_generation(generation);
        // `generation` is `Some` on exactly one road: a resume
        // (`spawn_resumed_session`). A fresh spawn starts unnumbered and the
        // journal numbers its first generation. The resumed generation is
        // mid-conversation, so the session owes no first prompt — the comment
        // on `first_prompt_owed` promises a resume never re-injects the
        // standing instructions into the next prompt the human sends.
        runtime.clear_first_prompt_owed();
    }
    if metadata.kind == SessionKind::Claude {
        let catalog = state.claude_models();
        runtime.store_claude_manifest(
            crate::claude_catalog::initial_manifest_with_mode(
                catalog.models,
                requested_mode.as_deref().unwrap_or("default"),
            ),
            catalog.state,
        );
    }
    if let Some(handle) = os_handle {
        runtime.install_os_handle(handle);
    }
    let process_job = Arc::new(process_job);
    {
        let registry = registry.clone();
        let owner = owner.clone();
        let session_id = metadata.id.clone();
        runtime.set_roster_notify(Arc::new(move || {
            registry.notify_session_transition(&owner, &session_id);
        }));
    }
    registry.configure_runtime_attention(&runtime, &owner);
    {
        // The finish report's trigger (`S5` §3; the slice-5 e2e battery is why
        // it is not the attention hook): a published `AgentFinished` calls this
        // once, and [`SessionRegistry::report_child_finish`] no-ops for a
        // session that is not an agent-created child of ours.
        let registry = registry.clone();
        let session_id = metadata.id.clone();
        runtime.set_finish_notify(Arc::new(move || {
            registry.report_child_finish(&session_id);
        }));
    }
    if metadata.kind.is_agent() {
        let death_killer = Mutex::new(killer.clone_killer());
        let job = Arc::clone(&process_job);
        runtime.set_on_os_death(Arc::new(move || {
            if let Ok(mut killer) = death_killer.lock() {
                killer.kill();
            }
            let _ = job.terminate();
        }));
    }
    let exited = Arc::new(AtomicBool::new(false));
    // Register before the reader thread starts: ConPTY's startup DSR can be
    // read within milliseconds, and the reply path needs the writer.
    if metadata.kind == SessionKind::Terminal {
        runtime
            .pty_writer
            .set(Arc::clone(&writer))
            .ok()
            .expect("pty writer registered exactly once");
    }
    let id = metadata.id.clone();
    let wait_id = id.clone();
    let wait_runtime = Arc::clone(&runtime);
    let wait_registry = registry.clone();
    let wait_owner = owner.clone();
    let child_wait = std::thread::Builder::new()
        .name(format!("session-wait-{id}"))
        .spawn(move || {
            let code = child.wait();
            wait_runtime
                .child_reaped
                .store(code.is_some(), Ordering::Release);
            wait_runtime.mark_exited(code);
            if wait_runtime.should_publish_exit_transition() {
                wait_registry.notify_session_transition(&wait_owner, &wait_id);
            }
            release_preserved_pty_after_drain(&wait_registry, &wait_id, EXIT_DRAIN);
            code
        })
        .ok();
    let session = PtySession {
        metadata,
        owner: owner.clone(),
        process_job,
        master,
        killer,
        steerer: switcher
            .as_ref()
            .map(|switcher| switcher.clone_steerer())
            .unwrap_or_else(|| Box::new(UnsupportedSteerer)),
        switcher,
        child_wait,
        writer,
        image_sink,
        static_image_sink,
        out_of_band,
        reader_handle: None,
        coalesce_handle: None,
        stderr_handle: None,
        runtime: Arc::clone(&runtime),
        mcp_session,
        exited: Arc::clone(&exited),
        preserve_on_exit: Arc::new(AtomicBool::new(false)),
    };

    // Insert BEFORE starting the reader. A shell can exit before the reader
    // thread gets scheduled; inserting later would let EOF cleanup miss the
    // map entry and strand the session.
    //
    // The entry goes in as `Configuring` and is promoted to `Live` only
    // after the delivery below has landed (the re-audit's P2-1). A child
    // that is live but not yet configured is the authority gap this slice
    // exists to close: between this insert and the delivery there used to be
    // a listed, promptable session whose card had not been honoured — a peer
    // could see it, send it work, and have that work silently die with a
    // refused delivery. A `Configuring` entry is invisible to every roster
    // read and refused by every id-addressed peer call, while the daemon's
    // own teardown paths (the refusal's `close`, EOF reaping) still reach
    // it.
    {
        let Ok(mut map) = registry.inner.lock() else {
            teardown_session(session);
            return Err(internal("Session state is unavailable."));
        };
        map.insert(id.clone(), RegistryEntry::Configuring(Box::new(session)));
    }

    let (coalesce_handle, reader_dispatch) = match reader_dispatch {
        Some(dispatch) => (None, dispatch),
        None => {
            let (coalesce_tx, coalesce_rx) = mpsc::channel::<Vec<u8>>();
            let coalesce_runtime = Arc::clone(&runtime);
            let coalesce_registry = registry.clone();
            let coalesce_session_id = id.clone();
            let coalesce_owner = owner.clone();
            let coalesce_handle = match std::thread::Builder::new()
                .name(format!("session-coalesce-{id}"))
                .spawn(move || {
                    coalesce_loop(
                        coalesce_rx,
                        coalesce_runtime,
                        coalesce_registry,
                        coalesce_session_id,
                        coalesce_owner,
                    )
                }) {
                Ok(handle) => Some(handle),
                Err(_) => {
                    let _ = registry.close(&id, &owner, &None);
                    return Err(WireError::new(
                        ErrorCode::Internal,
                        "Could not start the terminal reader.",
                    ));
                }
            };
            (
                coalesce_handle,
                Box::new(TerminalReaderDispatch {
                    tx: Some(coalesce_tx),
                }) as Box<dyn ReaderDispatch>,
            )
        }
    };

    let stderr_handle = stderr.and_then(|source| match source.spawn(Arc::clone(&runtime)) {
        Ok(handle) => Some(handle),
        Err(error) => {
            runtime.publish_agent_event(
                SessionEvent::AgentError {
                    message: format!("Could not drain agent stderr: {error}"),
                },
                None,
            );
            None
        }
    });
    if let Ok(mut map) = registry.inner.lock() {
        // The entry is `Configuring` until the delivery lands; the daemon's
        // own bookkeeping reaches through the window, peers do not.
        if let Some(session) = map
            .get_mut(&id)
            .and_then(RegistryEntry::as_child_process_mut)
        {
            session.coalesce_handle = coalesce_handle;
            session.stderr_handle = stderr_handle;
        }
    }

    let reader_registry = registry.clone();
    let reader_id = id.clone();
    let reader_runtime = Arc::clone(&runtime);
    let reader_state = Arc::downgrade(state);
    let reader_handle = match std::thread::Builder::new()
        .name(format!("session-pty-{id}"))
        .spawn(move || {
            reader_loop(
                reader_registry,
                reader_state,
                reader_id,
                reader,
                reader_runtime,
                reader_dispatch,
            );
        }) {
        Ok(handle) => handle,
        Err(_) => {
            let _ = registry.close(&id, &owner, &None);
            return Err(WireError::new(
                ErrorCode::Internal,
                "Could not start the terminal reader.",
            ));
        }
    };

    // The child can exit before this lock is acquired. In that case EOF
    // cleanup already removed the session; join the now-finished reader
    // here instead of leaking its handle.
    let mut orphaned_reader = Some(reader_handle);
    let mut orphaned_coalesce = None;
    if let Ok(mut map) = registry.inner.lock() {
        if let Some(session) = map
            .get_mut(&id)
            .and_then(RegistryEntry::as_child_process_mut)
        {
            session.reader_handle = orphaned_reader.take();
            session.coalesce_handle = orphaned_coalesce.take();
        }
    }
    if let Some(reader_handle) = orphaned_reader {
        let _ = reader_handle.join();
    }
    if let Some(coalesce_handle) = orphaned_coalesce {
        let _ = coalesce_handle.join();
    }
    // The pending delivery runs here and only here: it is an awaited rpc
    // whose answers only the session reader delivers, and that reader is now
    // live. Run any earlier and the wait outlives its deliverer — fifteen
    // seconds of stall, then a refusal, for every child a profile creates
    // (the R2a audit's F1). A refused delivery tears the child down — the
    // registry entry is still in its `Configuring` state, whose teardown the
    // close serves — and fails the creation, before any prompt can reach a
    // child the card did not describe.
    if let Some(deliver) = pending_delivery {
        if let Err(error) = deliver() {
            let _ = registry.close(&id, &owner, &None);
            return Err(error);
        }
    }
    // The delivery landed: the session exists. The promotion is one
    // critical section — remove and reinsert under the same lock hold — so
    // no other thread can observe the id absent, and from here on the
    // rosters list it and every id-addressed call reaches it.
    if let Ok(mut map) = registry.inner.lock() {
        if let Some(RegistryEntry::Configuring(session)) = map.remove(&id) {
            map.insert(id.clone(), RegistryEntry::Live(session));
        }
    }
    // S8 trigger: a Codex carrier verification runs detached — never blocking
    // this thread, never fatal whatever it answers. The reader above is live,
    // so the poll's answer has a deliverer; the flip lands whenever it lands
    // and the first prompt proceeds meanwhile. `None` on every road but the
    // carrier road (today: all of them).
    spawn_codex_verify_thread(pending_codex_verify, &runtime, &id);
    // A child can die before the create transition is published. Mark that
    // exit as covered by this first snapshot; the second check catches an
    // exit racing the publication without allowing the wait thread to report
    // the same transition twice.
    if runtime.process_exited() {
        runtime.exit_transition_sent.store(true, Ordering::Release);
    }
    registry.notify_session_transition(&owner, &id);
    runtime.transition_ready.store(true, Ordering::Release);
    if runtime.process_exited() && runtime.should_publish_exit_transition() {
        registry.notify_session_transition(&owner, &id);
    }
    Ok(())
}

/// S8 trigger body, one function so the test drives the real code: run a Codex
/// carrier verification detached and flip the runtime whenever it lands. `None`
/// is a no-op (today's only road). Detached, never blocking, never fatal — the
/// first prompt proceeds whatever the poll answers, and a late answer still
/// flips the roster (S8) whenever it arrives.
pub(crate) fn spawn_codex_verify_thread(
    bundle: Option<codex_client::CodexVerifyBundle>,
    runtime: &Arc<SessionRuntime>,
    session_id: &str,
) {
    let Some(bundle) = bundle else {
        return;
    };
    let verify_runtime = Arc::clone(runtime);
    let verify_id = session_id.to_string();
    let _ = std::thread::Builder::new()
        .name(format!("codex-verify-{verify_id}"))
        .spawn(move || {
            let state = codex_client::verify_codex_mcp(&bundle);
            verify_runtime.set_tools_state(state);
        });
}

fn reader_loop(
    registry: SessionRegistry,
    state: Weak<ServerState>,
    id: String,
    mut reader: Box<dyn Read + Send>,
    runtime: Arc<SessionRuntime>,
    mut reader_dispatch: Box<dyn ReaderDispatch>,
) {
    let mut buf = [0u8; READ_CHUNK];
    if let Err(error) = reader_dispatch.feed(&[], &runtime) {
        runtime.record_output_loss();
        runtime.fail_mcp_if_pending("The agent closed its output before the MCP broker was ready.");
        eprintln!("session {id} stopped before the first child read: {error}");
        reader_dispatch.finish(&runtime);
        return;
    }
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if let Err(error) = reader_dispatch.feed(&buf[..n], &runtime) {
                    runtime.record_output_loss();
                    eprintln!("session {id} stopped reading child output: {error}");
                    break;
                }
            }
            Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                runtime.record_output_loss();
                eprintln!("session {id} stopped reading terminal output: {error}");
                break;
            }
        }
    }
    reader_dispatch.finish(&runtime);
    runtime.fail_mcp_if_pending("The agent closed its output before the MCP broker was ready.");

    // EOF means the child ended. `stop` keeps the session object; `close`
    // and a natural exit remove it. session_finished is only for a removal
    // so a stopped-but-listed session still holds the idle-exit gate.
    let removed = finish_reader_session(&registry, &id, &runtime);
    runtime.reader_finished.store(true, Ordering::Release);
    if removed {
        if let Some(state) = state.upgrade() {
            state.session_finished();
        }
    }
}

fn coalesce_loop(
    rx: mpsc::Receiver<Vec<u8>>,
    runtime: Arc<SessionRuntime>,
    registry: SessionRegistry,
    session_id: String,
    owner: OwnerId,
) {
    let mut pending = Vec::new();
    loop {
        let received = if pending.is_empty() {
            rx.recv().ok()
        } else {
            match rx.recv_timeout(COALESCE_FLUSH) {
                Ok(bytes) => Some(bytes),
                Err(RecvTimeoutError::Timeout) => {
                    flush_coalesced(&mut pending, &runtime, &registry, &session_id, &owner);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => None,
            }
        };
        match received {
            Some(bytes) => {
                pending.extend_from_slice(&bytes);
                if pending.len() >= COALESCE_MAX_BYTES || pending.len() == COALESCE_EAGER_BYTES {
                    flush_coalesced(&mut pending, &runtime, &registry, &session_id, &owner);
                }
            }
            None => {
                flush_coalesced(&mut pending, &runtime, &registry, &session_id, &owner);
                break;
            }
        }
    }
}

fn flush_coalesced(
    pending: &mut Vec<u8>,
    runtime: &SessionRuntime,
    registry: &SessionRegistry,
    session_id: &str,
    owner: &OwnerId,
) {
    if pending.is_empty() {
        return;
    }
    let data = String::from_utf8_lossy(pending).into_owned();
    pending.clear();
    if runtime.publish_output(&data) && runtime.transition_ready() {
        registry.notify_session_transition(owner, session_id);
    }
}

/// Returns whether the registry entry was removed (so the caller can
/// decrement the live-session count). `None` from the lock means another
/// path already took the session — do not session_finished again.
pub(super) fn finish_reader_session(
    registry: &SessionRegistry,
    id: &str,
    runtime: &SessionRuntime,
) -> bool {
    let Ok(mut map) = registry.inner.lock() else {
        return false;
    };
    // Captured before the mutable borrow below: a preserved session stays in
    // the map, and its end still owes its creator a report (audit S5B-03).
    let owner = map.get(id).map(|entry| entry.owner().clone());
    let Some(session) = map
        .get_mut(id)
        .and_then(RegistryEntry::as_child_process_mut)
    else {
        return false;
    };
    session.reader_handle = None;
    let preserve = session.preserve_on_exit.load(Ordering::SeqCst);
    if preserve {
        let coalesce = session.coalesce_handle.take();
        let mcp_session = session.mcp_session.take();
        session.exited.store(true, Ordering::SeqCst);
        // A preserved terminal keeps its row and transcript for History, but
        // its master-side handles keep the ConPTY host alive: a stopped
        // terminal must not leave a headless conhost behind. Agents hold no
        // master and keep their pipes.
        let released = (session.metadata.kind == SessionKind::Terminal).then(|| {
            let master = session.master.take();
            ReleasedPtyMaster {
                master,
                writer: Arc::clone(&session.writer),
            }
        });
        let ended = owner.clone().map(|owner| {
            (
                live_session_view(session),
                Arc::clone(&session.runtime),
                owner,
            )
        });
        drop(map);
        // Released outside the registry lock: closing the pseudoconsole must
        // not happen under the map lock, the same reason teardown_session
        // drops its handles only after the removal.
        if let Some(released) = released {
            released.close();
        }
        drop(mcp_session);
        join_coalesce(coalesce, runtime);
        journal_mark_ended(registry, runtime);
        runtime.close_output();
        // A stopped child that kept its transcript is a child that ended (audit
        // S5B-03): the session stays listed on purpose, and its slot and its
        // report are still owed to the creator.
        if let Some((session, child_runtime, owner)) = ended {
            // A child whose creation has not committed yet has no link and a
            // creator that does not know about it: the end waits for the
            // commit (audit-2 §2).
            if !registry.defer_child_end_if_pending(id, &session, &child_runtime, &owner) {
                registry.child_ended_with(id, Some(&session), Some(&child_runtime), Some(&owner));
            }
        }
        return false;
    }
    // The target's message-brake entries leave with it (A2-06), inside this
    // same critical section: an admission that found the session in the map
    // cannot reserve a slot for it after this point (A2-05).
    forget_message_brake_target(&registry.message_brakes, id);
    // What this child's end owes its creator is copied out of the row *before*
    // it is removed (`S5` decisions 7 and 8, audit S5-01): the report needs the
    // row's metadata, its runtime and its owner, and this is the path that ends
    // a child whose provider exited on its own — the common end — which used to
    // take the row out without releasing the slot or telling the creator.
    let ended = map.get(id).and_then(|entry| {
        entry.as_child_process().map(|live| {
            (
                live_session_view(live),
                Arc::clone(&live.runtime),
                entry.owner().clone(),
            )
        })
    });
    // `Configuring` is taken too: a child whose delivery never landed is
    // still a child whose end owes the teardown below.
    let (Some(RegistryEntry::Live(session)) | Some(RegistryEntry::Configuring(session))) =
        map.remove(id)
    else {
        return false;
    };
    let mut session = *session;
    drop(map);
    session.reader_handle = None;
    let coalesce = session.coalesce_handle.take();
    let stderr = session.stderr_handle.take();
    let child_wait = session.child_wait.take();
    let PtySession {
        master,
        writer,
        image_sink: _,
        static_image_sink: _,
        killer,
        runtime: session_runtime,
        mcp_session,
        exited,
        ..
    } = session;
    exited.store(true, Ordering::SeqCst);
    // Revoke MCP before killing the child: an in-flight provider request may
    // race teardown, and a closed session must not authorize new work.
    drop(mcp_session);
    drop(killer);
    drop(writer);
    drop(master);
    bounded_join(stderr);
    bounded_join(child_wait);
    join_coalesce(coalesce, runtime);
    let _ = session_runtime;
    journal_mark_ended(registry, runtime);
    runtime.close_output();
    // The end is complete: the report (once per child, whatever path got here
    // first) and the release of the child's slot, in that order.
    if let Some((session, child_runtime, owner)) = ended {
        // The same deferral as the preserve branch: an end that beats the
        // commit waits for it (audit-2 §2).
        if !registry.defer_child_end_if_pending(id, &session, &child_runtime, &owner) {
            registry.child_ended_with(id, Some(&session), Some(&child_runtime), Some(&owner));
        }
    }
    true
}

/// The master-side handles of a preserved terminal, taken when its child has
/// ended. The ConPTY host lives until both are gone — the pseudoconsole
/// owner and the input-pipe writer — so `close` gives them up in the order
/// teardown uses: the writer first, then the master whose drop closes the
/// pseudoconsole and ends the host.
struct ReleasedPtyMaster {
    master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl ReleasedPtyMaster {
    fn close(self) {
        if let Ok(mut writer) = self.writer.lock() {
            *writer = Box::new(ClosedPtyWriter);
        }
        drop(self.master);
    }
}

/// The writer left in a preserved terminal's slot once its pipe is gone.
/// Writes fail the way the closed pipe would, so a late prompt gets an
/// honest refusal instead of vanishing into a dead host.
struct ClosedPtyWriter;

impl Write for ClosedPtyWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "the terminal was archived",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "the terminal was archived",
        ))
    }
}

/// Release a preserved terminal's master-side handles once its child has
/// been reaped. The kill road needs this because its reader never gets
/// there: the killed shell's ConPTY host keeps the pipes open and the
/// reader stays blocked, so the preserve branch at EOF never runs (the
/// journal schema's own reaped-but-still-live sweep is the same hole seen
/// from a daemon restart). After `drain` — the same budget the exit
/// announcement gives the reader to pump the last bytes — take the handles
/// under one hold of the map lock and close them outside it. A second
/// read of `preserve_on_exit` under that hold keeps a resume's replacement
/// entry, which never inherits the flag, out of the release.
pub(super) fn release_preserved_pty_after_drain(
    registry: &SessionRegistry,
    id: &str,
    drain: Duration,
) {
    let preserved_terminal = |map: &HashMap<String, RegistryEntry>| {
        map.get(id)
            .and_then(RegistryEntry::as_child_process)
            .is_some_and(|session| {
                session.metadata.kind == SessionKind::Terminal
                    && session.preserve_on_exit.load(Ordering::SeqCst)
            })
    };
    {
        let Ok(map) = registry.inner.lock() else {
            return;
        };
        if !preserved_terminal(&map) {
            return;
        }
    }
    std::thread::sleep(drain);
    let released = {
        let Ok(mut map) = registry.inner.lock() else {
            return;
        };
        if !preserved_terminal(&map) {
            return;
        }
        let Some(session) = map
            .get_mut(id)
            .and_then(RegistryEntry::as_child_process_mut)
        else {
            return;
        };
        let master = session.master.take();
        Some(ReleasedPtyMaster {
            master,
            writer: Arc::clone(&session.writer),
        })
    };
    if let Some(released) = released {
        released.close();
    }
}

/// End a stillborn row without stalling the dispatch thread. The blocking
/// send is an unbounded 5 ms busy-loop, so a create/resume failure rides a
/// throwaway thread; the row still ends once the queue drains and the
/// revision bump wakes roster readers then.
pub(super) fn spawn_async_end_marker(journal: &Arc<Journal>, session_id: &str, generation: u64) {
    let journal = Arc::clone(journal);
    let id = session_id.to_string();
    let _ = std::thread::Builder::new()
        .name("journal-end-marker".into())
        .spawn(move || {
            let _ = journal.mark_ended_blocking(&id, generation, None);
        });
}

fn journal_mark_ended(registry: &SessionRegistry, runtime: &SessionRuntime) {
    let Some(journal) = &registry.journal else {
        return;
    };
    let (generation, code) = match runtime.lock_stream() {
        Ok(stream) => (stream.generation, stream.exit_code),
        Err(_) => (runtime.generation(), None),
    };
    // EOF path: waiting on the journal here does not stall a live PTY. The
    // terminal marker is critical and must not be dropped behind a full
    // output queue.
    if let Err(error) = journal.mark_ended_blocking(&runtime.session_id, generation, code) {
        runtime.mark_journal_degraded();
        eprintln!(
            "journal could not record terminal exit for {}: {error}",
            runtime.session_id
        );
    }
    registry.invalidate_journal_roster();
}

pub(super) fn terminate_spawned_child(
    pair: portable_pty::PtyPair,
    mut child: Box<dyn Child + Send + Sync>,
) {
    let mut killer = child.clone_killer();
    let _ = killer.kill();
    drop(pair.master);
    let _ = child.wait();
}

/// Kill + drop writer/master + wait + bounded reader join. ORDER IS
/// LOAD-BEARING: on Windows, waiting while the ConPTY master is alive can
/// deadlock the ConPTY host. Dropping the master also unblocks the
/// reader's blocking read.
pub(super) fn teardown_session(session: PtySession) {
    teardown_session_inner(session, true);
}

/// Tear down a replaced provider generation without marking the journal row
/// ended. `resume` immediately starts the next generation on this same row.
pub(super) fn teardown_session_for_resume(session: PtySession) {
    teardown_session_inner(session, false);
}

fn teardown_session_inner(session: PtySession, finish_runtime: bool) {
    session.exited.store(true, Ordering::SeqCst);
    session
        .runtime
        .fail_mcp_if_pending("The agent session closed before the MCP broker was ready.");
    let PtySession {
        process_job,
        master,
        mut killer,
        steerer: _,
        switcher: _,
        child_wait,
        writer,
        image_sink: _,
        static_image_sink: _,
        out_of_band: _,
        reader_handle,
        coalesce_handle,
        stderr_handle,
        runtime,
        exited: _,
        owner: _,
        metadata: _,
        preserve_on_exit: _,
        mcp_session,
    } = session;

    // Revocation intentionally precedes child death. The provider may still
    // have an in-flight request, but the closed session must already be
    // unauthorized by the time teardown starts.
    drop(mcp_session);
    // 1) Kill first. The killer is separate so this cannot race with wait().
    killer.kill();
    drop(killer);
    // 2) Drop writer and master BEFORE wait(). The writer owns another
    //    master-side handle, and ConPTY's host can remain alive while either
    //    handle is open. Closing them also unblocks the reader. The registry
    //    entry was removed before this function, so only transient
    //    command-side Arc clones remain.
    drop(writer);
    drop(master);
    // The job must end before the bounded joins, and by terminate rather
    // than by handle close: an agent's on_os_death callback owns another
    // Arc to this job inside the runtime, so a reader that outlives its
    // join budget keeps the job — and any grandchild still holding the
    // pipes — alive past this function. TerminateJobObject is asynchronous,
    // so the bounded wait is what makes "the tree is dead before the
    // joins" true: the wait budget is the same one the joins get. If the
    // wait fails, that other Arc is what would keep the fallback below
    // from ever running, so the callback is released next.
    if let Err(error) = process_job.terminate_and_wait(READER_JOIN_BUDGET) {
        eprintln!(
            "session {} could not terminate its job before teardown joins: {error}",
            runtime.session_id
        );
        release_after_failed_wait(&runtime);
    }
    drop(process_job);
    // 3) Reap after the PTY endpoints are closed; this prevents a zombie
    //    and avoids the Windows ConPTY wait deadlock. The waiter thread
    //    owns Child::wait so we join it here instead of calling wait()
    //    ourselves.
    bounded_join(child_wait);
    bounded_join(stderr_handle);
    // 4) Best-effort bounded join. JoinHandle has no timed join; the
    //    endpoint close above makes the reader finish promptly, while this
    //    small budget prevents shutdown from accumulating a hang across
    //    sessions.
    // The order above is intentional: the coalescer gets every chance to
    // publish before output is closed. If its bounded join still gives up,
    // any pending bytes may be discarded by the later finish(). Surface that
    // loss through the same per-session degradation signal used for journal
    // failures instead of silently claiming completeness.
    join_coalesce(coalesce_handle, &runtime);
    bounded_join(reader_handle);
    if finish_runtime {
        runtime.finish(None);
    }
}

/// Teardown's step after a failed job termination, before the caller drops
/// its own job `Arc`: the callback is released so that drop *can* be the
/// job's last handle. `fire_os_death`'s detached thread and `stop` may
/// still hold the job `Arc`, so the close this drop misses arrives with
/// the last of them — every holder runs its own `terminate()`, which is
/// why a deferred close is deferred, not lost. Production calls this from
/// `teardown_session_inner` and the seam test calls this same step, so
/// deleting the release inside it fails the suite.
pub(super) fn release_after_failed_wait(runtime: &SessionRuntime) {
    runtime.release_on_os_death();
}

fn join_coalesce(handle: Option<JoinHandle<()>>, runtime: &SessionRuntime) {
    if !bounded_join(handle) {
        runtime.mark_journal_degraded();
        eprintln!(
            "session {} coalesce thread exceeded teardown join budget; scrollback may be truncated",
            runtime.session_id
        );
    }
}

fn bounded_join<T>(handle: Option<JoinHandle<T>>) -> bool {
    if let Some(handle) = handle {
        let deadline = Instant::now() + READER_JOIN_BUDGET;
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if handle.is_finished() {
            let _ = handle.join();
            return true;
        }
        return false;
    }
    true
}

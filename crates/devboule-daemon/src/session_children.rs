//! The agent creation table and the child lifecycle: the idempotency keys a
//! creation holds, the child readmit/release/notice/report roads, the quiet
//! sweeper and the finish report.
//!
//! Split out of `session.rs` without a rewrite: every line below this header is
//! byte-identical to its text there, apart from the `pub(super)` markers on the
//! methods a caller in the parent module or its sibling tests reaches in for.

use super::*;

impl super::SessionRegistry {
    /// Hold one creation's idempotency key for as long as the call that claimed
    /// it runs (audit S5-03).
    ///
    /// Taken *before* the idempotency store is read, so a second call with the
    /// same key — a client that re-sent while the first is still raising a card
    /// — is refused with `creation in progress; retry` and spends nothing. The
    /// guard hands the key back when it is dropped, so every refusal between
    /// here and the answer releases it without a cleanup line per path.
    pub(crate) fn hold_creation_key<'a>(
        &'a self,
        key: &str,
    ) -> Result<CreationKeyHold<'a>, WireError> {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !table.begin_creation(key, Instant::now()) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "creation in progress; retry",
            ));
        }
        Ok(CreationKeyHold {
            sessions: self,
            key: Some(key.to_string()),
        })
    }

    /// The creation this key was held for is over, either way: its result is in
    /// the idempotency store, or it failed and stored nothing.
    pub(crate) fn end_agent_creation(&self, key: &str) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.end_creation(key);
    }

    /// Take one creation slot for `creator`, or say why not (`S5` decision 5).
    ///
    /// The slot is taken *before* the card is raised and before anything is
    /// spawned, which is what makes the caps hold under two creations racing on
    /// one session: an admission that later fails releases it
    /// ([`Self::release_agent_creation`]) and one that succeeds commits it
    /// ([`Self::commit_agent_creation`]).
    ///
    /// `depth` is the child's depth — the creator's own, plus one — and comes
    /// from the caller's MCP registration, which the daemon wrote when that
    /// session was created. A caller-supplied depth is not accepted anywhere.
    pub(crate) fn reserve_agent_creation(
        &self,
        creator: &str,
        depth: u32,
    ) -> Result<AgentCreationTicket<'_>, WireError> {
        if depth > MAX_AGENT_DEPTH {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "depth limit; do not retry",
            ));
        }
        let now = Instant::now();
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if table.sweep_is_due(now) {
            table.sweep(now);
        }
        if table.live_agent_sessions() >= MAX_LIVE_AGENT_SESSIONS {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "creation limit exceeded; do not retry",
            ));
        }
        let reservation = table.next_reservation;
        table.next_reservation += 1;
        let caps = table
            .creators
            .entry(creator.to_string())
            .or_insert_with(|| AgentCreatorCaps::new(now));
        caps.roll_window(now);
        if caps.held() >= MAX_LIVE_CHILDREN_PER_CREATOR {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "creation limit exceeded; do not retry",
            ));
        }
        if caps.creations_in_window >= MAX_CREATIONS_PER_WINDOW {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "creation limit exceeded; do not retry",
            ));
        }
        // The once-per-session card, decided here rather than by the caller
        // (audit S5-06). A card that is already with the human blocks this
        // caller *before* it spends a slot: it is not a refusal the caller can
        // act on by retrying something else, it is "wait for the answer".
        let card_owed = match caps.gate {
            CreationGate::Pending => {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "creation permission pending; retry",
                ))
            }
            CreationGate::Closed => {
                caps.gate = CreationGate::Pending;
                true
            }
            CreationGate::Open => false,
        };
        // The child's id is reserved here rather than inside the spawn
        // (audit S5B-04): the link below names it, and the link has to exist
        // before the process does, because a provider that exits on the
        // instant would otherwise reach EOF with nothing to release.
        let caps = table
            .creators
            .get_mut(creator)
            .expect("the entry taken for this reservation");
        // The reservation carries no child id: the spawn composes that one, and
        // the link is registered at the commit under the id the child really
        // has. Registering it here instead was tried in this pass and closed
        // the child's own transport before its handshake (see the report).
        caps.in_flight.insert(reservation, String::new());
        caps.creations_in_window += 1;
        let child = String::new();
        Ok(AgentCreationTicket {
            registry: self,
            creator: creator.to_string(),
            reservation,
            child,
            card_owed,
            committed: false,
            caps: devboule_protocol::CreateAgentCaps {
                // The numbers the card states are what the budget reads
                // *including* the creation being asked about: the human is
                // deciding whether to spend this slot, so it is counted.
                live_children: caps.held() as u32,
                max_live_children: MAX_LIVE_CHILDREN_PER_CREATOR as u32,
                creations_this_hour: caps.creations_in_window,
                max_creations_per_hour: MAX_CREATIONS_PER_WINDOW,
                depth,
                max_depth: MAX_AGENT_DEPTH,
                live_agent_sessions: table.live_agent_sessions() as u32,
                max_live_agent_sessions: MAX_LIVE_AGENT_SESSIONS as u32,
            },
        })
    }

    /// The human allowed this creator to create: the once-per-creator-session
    /// gate opens and stays open for as long as the entry lives (`S5` decision
    /// 4, S5-06).
    pub(crate) fn accept_agent_creation(&self, creator: &str) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(caps) = table.creators.get_mut(creator) {
            caps.gate = CreationGate::Open;
        }
    }

    /// Release one reservation by identity, for a caller that holds the id
    /// rather than the ticket (the tests, and the rollback paths that need to
    /// know whether anything was still outstanding).
    ///
    /// The ticket's `Drop` is the normal way in; this answers `false` for an
    /// id that is not outstanding, which is what makes a double release a
    /// no-op (audit S5B-02).
    pub(crate) fn release_agent_creation(&self, creator: &str, reservation: u64) -> bool {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(child) = table
            .creators
            .get_mut(creator)
            .and_then(|caps| caps.in_flight.remove(&reservation))
        else {
            return false;
        };
        // A marker belongs to the reservation that noted it and goes with it
        // (audit-3 S5D-01): the sweep is no longer a backstop for a marker, so
        // this release is the last of the four paths that end one.
        table
            .pending_children
            .retain(|_, (_, owned_by)| *owned_by != reservation);
        {
            let caps = table.creators.get_mut(creator).expect("the entry above");
            caps.creations_in_window = caps.creations_in_window.saturating_sub(1);
            if caps.gate == CreationGate::Pending {
                caps.gate = CreationGate::Closed;
            }
        }
        if table.children.get(&child).is_some_and(|link| !link.started) {
            table.children.remove(&child);
        }
        let drop_creator = table
            .creators
            .get(creator)
            .is_some_and(|caps| caps.creator_gone && caps.held() == 0);
        if drop_creator {
            table.creators.remove(creator);
        }
        true
    }

    /// A resumed child is a child again (audit S5B-05).
    ///
    /// With its creator live, the session re-enters the children table: the cap
    /// counts one child, not none, and a later end releases the slot and reports
    /// through the same routine as any other child. A creator that is gone, or
    /// an entry already marked gone, leaves the session ordinary — the roster
    /// still names the parent, and the caps ignore it.
    ///
    /// The *depth* is not persisted (the journal has no column for it, and
    /// adding one is a migration of its own): a resumed child comes back at
    /// depth 1. `notify` comes back `true` for the same reason — the report is
    /// what the link is for, and a resumed child that could end in silence
    /// would be the worse surprise.
    pub(super) fn readmit_agent_child(&self, child: &str, creator: Option<&str>, owner: &OwnerId) {
        let Some(creator) = creator else {
            return;
        };
        if self.live_runtime(creator, owner).is_none() {
            return;
        }
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if table
            .creators
            .get(creator)
            .is_some_and(|caps| caps.creator_gone)
        {
            return;
        }
        // Resuming the same child again must not count it again (audit S5B-05):
        // the link is written once, and only a child that was *not* linked
        // spends a slot here.
        if table.children.contains_key(child) {
            return;
        }
        table.children.insert(
            child.to_string(),
            AgentChild {
                creator: creator.to_string(),
                notify: true,
                started: true,
                notice_owed: true,
                report_owed: true,
                quiet_notified: false,
                idle_close_since: None,
                idle_close_notified: false,
            },
        );
        let caps = table
            .creators
            .entry(creator.to_string())
            .or_insert_with(|| AgentCreatorCaps::new(Instant::now()));
        caps.live_children += 1;
    }

    /// Reserve with the owner a test does not care about, keeping the cap
    /// tests readable now that a reservation carries an identity.
    #[cfg(test)]
    pub(super) fn test_ticket(
        &self,
        creator: &str,
        depth: u32,
    ) -> Result<AgentCreationTicket<'_>, WireError> {
        self.reserve_agent_creation(creator, depth)
    }

    /// Register a child the test named itself, the way the spawn path does for
    /// a real one. The newest reservation, if any, becomes that child.
    #[cfg(test)]
    pub(super) fn commit_agent_child_for_test(&self, creator: &str, child: &str, notify: bool) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // The reservation's own link (registered under the id the reservation
        // composed) goes: the child this call names takes its place, so the
        // budget sees one child either way and nothing is leaked.
        let reserved = table
            .creators
            .get_mut(creator)
            .and_then(|caps| caps.in_flight.keys().next_back().copied());
        if let Some(reserved) = reserved {
            if let Some(caps) = table.creators.get_mut(creator) {
                if let Some(composed) = caps.in_flight.remove(&reserved) {
                    table.children.remove(&composed);
                }
            }
            if let Some(caps) = table.creators.get_mut(creator) {
                caps.live_children += 1;
            }
        } else if let Some(caps) = table.creators.get_mut(creator) {
            caps.live_children += 1;
        }
        table.children.insert(
            child.to_string(),
            AgentChild {
                creator: creator.to_string(),
                notify,
                started: true,
                notice_owed: true,
                report_owed: true,
                quiet_notified: false,
                idle_close_since: None,
                idle_close_notified: false,
            },
        );
    }

    /// The tests reason in "this creator's newest reservation": release it.
    #[cfg(test)]
    pub(super) fn abandon_agent_creation_for_test(&self, creator: &str) {
        let newest = {
            let table = self
                .creations
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            table
                .creators
                .get(creator)
                .and_then(|caps| caps.in_flight.keys().next_back().copied())
        };
        if let Some(reservation) = newest {
            assert!(
                self.release_agent_creation(creator, reservation),
                "the newest reservation of {creator}"
            );
        }
    }

    /// The creator session is gone: its own entry follows its last child out.
    ///
    /// Called from `close`, so a session that ends normally is forgotten here
    /// rather than by the sweep — the sweep is the backstop for entries whose
    /// creator vanished without one, and it is the reason the table cannot grow
    /// without bound.
    pub(crate) fn forget_agent_creator(&self, creator: &str) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(caps) = table.creators.get_mut(creator) {
            caps.creator_gone = true;
            if caps.held() == 0 {
                table.creators.remove(creator);
            }
        }
    }

    /// Create the child an agent asked for, from the preset's own answers.
    ///
    /// Reuses the `SessionCreate` path end to end — the same spawn, the same
    /// `register_with_provider`, the same journal row — with the four facts a
    /// client can never supply: the creator's origin, the creator's owner, this
    /// session's parent and depth, and the preset's tool overlay. The bounds
    /// (the title, the workspace, the prompt) were enforced by the broker
    /// before this is called; nothing here is asked of the caller again.
    pub(crate) fn create_session_for_agent(
        &self,
        state: &Arc<ServerState>,
        creation: AgentCreation,
        ticket: AgentCreationTicket<'_>,
    ) -> Result<Session, WireError> {
        // The wire create road's gate (`server/sessions.rs`), for the road that
        // has no client frame: an agent child is a live daemon-owned session,
        // and its entry holds an idle-shutdown slot from here until a removal
        // path gives it back.
        if !state.session_started() {
            return Err(WireError::new(
                ErrorCode::ShuttingDown,
                "daemon is shutting down",
            ));
        }
        let creator_owner = creation.creator.owner.clone();
        let mut meta = SessionCreateMeta::for_agent_child(
            &creation.creator_session_id,
            &creation.creator.origin,
            &creation.display_name,
            creation.depth,
            creation.overlay,
            creation.cwd.clone(),
        );
        // The marker the spawn notes carries this reservation (audit-3 S5D-01),
        // so the reservation's own release clears it as surely as the commit and
        // the abandon do: no path can leave a marker behind its creation.
        meta.reservation = Some(ticket.reservation());
        // The creation-from-profile facts, on the same meta the reservation
        // travels on: one place describes a child's birth. The mode and the
        // overlay already went through `for_agent_child` above; these are
        // what the profile added to the creation, and none of them is
        // re-derived later — the row keeps what the birth measured. The
        // marker itself is derived inside `create_with_provider_env` from the
        // delivery this creation carries, which is the same mode the child
        // will actually be started in.
        meta.profile_id = Some(creation.profile_id.clone());
        meta.labels = creation.labels.clone();
        meta.context_id = creation.context_id.clone();
        // The id the reservation already registered a link for (audit S5B-04):
        // the spawn must use it, so an exit on the instant finds the row that
        // releases the slot and reports the end.
        // `SessionCreateMeta::session_id` stays None: the spawn composes the
        // child's id (see `commit_agent_creation`).
        let kind = crate::provider_catalog::session_kind_for(&creation.provider);
        let child = match self.create_with_provider_env(
            state,
            &creation.creator.owner,
            creation.workspace_id.clone(),
            kind,
            Some(creation.provider.clone()),
            creation.delivery.clone(),
            None,
            // The MCP connection is not a client connection: every ownership
            // check below uses the creator's own owner, and the origin was
            // passed explicitly rather than derived from this.
            &None,
            None,
            &meta,
        ) {
            Ok(child) => child,
            Err(error) => {
                // The creation failed: the slot taken at the gate goes back,
                // as it does on the wire create road's own `Err` arm
                // (`server/sessions.rs`).
                state.session_finished();
                return Err(error);
            }
        };
        // The ticket's `Drop` releases the reservation; the journal row a failed
        // spawn leaves behind is ended by `create_with_provider_env` itself.
        let (committed, deferred) = self.commit_agent_creation(
            &creation.creator_session_id,
            ticket.reservation(),
            &child.id,
            creation.notify,
        );
        if !committed {
            // The creator closed while its child was starting (audit S5B-09):
            // a session nobody owns is not a creation that succeeded, so the
            // child is closed again and the caller is told why. The ticket's
            // `Drop` gives the reservation back. The close is not the wire
            // handler that releases the slot, so the release is here — and
            // only when it really removed the entry, because an end that beat
            // it released the slot already.
            if self.abandon_uncommitted_child(&child.id, &creator_owner) {
                state.session_finished();
            }
            return Err(WireError::new(ErrorCode::InvalidRequest, "creator closed"));
        }
        // The reservation is a child now: nothing is given back on this path.
        ticket.commit();
        // The creation is recorded on the *creator's* transcript, beside the
        // children the roster names: the child's own transcript begins with its
        // prompt and must not explain where it came from.
        //
        // Before the prompt, not after (audit S5-10): a child that answers and
        // ends inside the write would otherwise reach the creator as a finish
        // with no creation in front of it. Publishing first also means the
        // creator has a name for the child before anything can end it, so a
        // prompt that fails to write closes a session the human can see.
        // The handle is the one the card was raised through, taken before the
        // spawn (audit-2 §1): looking the creator up again afterwards can miss
        // it (a closed or replaced entry) and the creation record would vanish
        // with it, taking S5-10's ordering guarantee with it.
        let creator_runtime = creation
            .creator_runtime
            .clone()
            .or_else(|| self.live_runtime(&creation.creator_session_id, &creator_owner));
        self.publish_child_created_then_end(
            creator_runtime.as_ref(),
            &child.id,
            &creation.display_name,
            &creation.provider,
            &creation.profile_name,
            deferred,
        );
        // The child's first prompt. The preset preamble is no longer glued here:
        // it travels as `preset_preamble` and is composed by the send path, in one
        // place with the human's standing instructions in front of it
        // (`compose_first_prompt`), so every provider receives one string built by
        // one rule. The resolved profile's spawn prompt rides beside it, in its
        // fixed place between the standing instructions and the preamble; an
        // empty one is the absent one, and it was read once when the creation
        // was resolved — a later profile edit cannot reach this child.
        let prompt = creation.initial_prompt.clone();
        let spawn_prompt = creation.spawn_prompt.clone();
        let owner = creation.creator.owner.clone();
        let internal_conn = ConnHandle::with_peer(0, None);
        let sent = self.send_with_subscription_timeout(&SendRequest {
            session_id: &child.id,
            subscription_id: 0,
            text: &prompt,
            attachments: &[],
            // Empty by construction: the standing instructions, the spawn
            // prompt, the preamble and the caller's text are the whole
            // prompt, and `devboule_create_agent` has no parameter that names
            // a stored attachment.
            attachment_references: &[],
            owner: &owner,
            conn: &internal_conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: false,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: Some(crate::provider_catalog::AGENT_PREAMBLE),
            spawn_prompt: (!spawn_prompt.is_empty()).then_some(spawn_prompt.as_str()),
            author: UserMessageAuthor::Creation,
            message_kind: UserMessageKind::Creation,
        });
        if let Err(error) = sent {
            // The same pairing as the abandon above: the close removes an
            // entry this road counted, and it is not the wire handler.
            if self.close(&child.id, &owner, &None).unwrap_or(false) {
                state.session_finished();
            }
            return Err(error);
        }
        Ok(child)
    }

    /// The child exists: its reservation becomes a live child (audit S5B-02),
    /// or the commit is refused because the creator is gone (audit S5B-09).
    ///
    /// `false` means the creator closed while its child was starting. The
    /// caller then closes the child again: a session nobody owns is not a
    /// creation that succeeded, and the caller's `Drop` releases the
    /// reservation.
    pub(super) fn commit_agent_creation(
        &self,
        creator: &str,
        reservation: u64,
        child: &str,
        notify: bool,
    ) -> (bool, Option<DeferredChildEnd>) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(caps) = table.creators.get_mut(creator) else {
            return (false, None);
        };
        if caps.creator_gone {
            return (false, None);
        }
        let Some(reserved) = caps.in_flight.remove(&reservation) else {
            return (false, None);
        };
        // The reservation was registered without a child id (the spawn composes
        // that one): the link this child gets is registered here, and it is the
        // link every later end path finds.
        debug_assert!(reserved.is_empty(), "a reservation never names a child");
        caps.live_children += 1;
        // An end that arrived before this link existed runs now, on this
        // thread, exactly as if the order had been the other way round.
        table.pending_children.remove(child);
        let deferred = table.deferred_child_ends.remove(child).map(|(end, _)| end);
        table.children.insert(
            child.to_string(),
            AgentChild {
                creator: creator.to_string(),
                notify,
                started: true,
                notice_owed: true,
                report_owed: true,
                quiet_notified: false,
                idle_close_since: None,
                idle_close_notified: false,
            },
        );
        (true, deferred)
    }

    /// A child has ended, whatever ended it (`S5` decisions 7 and 8; audit
    /// S5-01).
    ///
    /// One routine, called from every path that can take a child out of the
    /// live map, so the caps row is released and the finish report is produced
    /// on the *first* of them and on no later one:
    ///
    /// * `close` — the explicit close, with the row it just removed;
    /// * [`Self::finish_reader_session`] — the reader's EOF, which is also how a
    ///   process exit is observed, with the row it just removed;
    /// * `resume` — the live entry a resume replaces;
    ///
    /// and the runtime's transition notify ([`Self::report_child_events`])
    /// *reports* without releasing, because the session is still live there.
    ///
    /// It takes what the caller still has in hand rather than looking the row up
    /// again: the paths that ended the child have already removed it.
    /// Note that this child's creation has not committed yet (audit-2 §2).
    /// The spawn noted the child it is starting: until the commit (or the
    /// abandon) an end that beats it is parked instead of lost. The marker
    /// carries the reservation that owns it, which is its whole lifetime
    /// (audit-3 S5D-01) — the sweep does not age it.
    pub(crate) fn note_pending_child(&self, child: &str, reservation: u64) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table
            .pending_children
            .insert(child.to_string(), (Instant::now(), reservation));
    }

    /// The creation never got as far as a link: nothing is owed anywhere.
    /// A creation that did not commit because its creator is gone (audit-3
    /// §2): the child is closed **and** nothing its start recorded outlives it.
    ///
    /// The clear comes first and under its own lock: the close can end the child,
    /// and an end that found the marker still set would park a second deferred
    /// entry that no commit is left to consume.
    /// Answers whether the close removed a live entry, which is what the
    /// caller's slot release pairs with; a refusal removed nothing.
    pub(super) fn abandon_uncommitted_child(&self, child: &str, owner: &OwnerId) -> bool {
        self.clear_pending_child(child);
        self.close(child, owner, &None).unwrap_or(false)
    }

    pub(crate) fn clear_pending_child(&self, child: &str) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.pending_children.remove(child);
        table.deferred_child_ends.remove(child);
    }

    /// Park an end that arrived first, **and decide that under the same lock**
    /// (audit-3 §1).
    ///
    /// `true` means the end is parked and the commit will run it. `false` means
    /// this child's creation has already committed (or its marker was taken
    /// away), so the caller runs the routine itself — the check and the insert
    /// are one critical section, which is what keeps a commit from landing
    /// between them and leaving a parked end with no consumer.
    pub(super) fn defer_child_end_if_pending(
        &self,
        child: &str,
        session: &Session,
        runtime: &Arc<SessionRuntime>,
        owner: &OwnerId,
    ) -> bool {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !table.pending_children.contains_key(child) {
            return false;
        }
        table.deferred_child_ends.insert(
            child.to_string(),
            (
                (
                    Some(session.clone()),
                    Some(Arc::clone(runtime)),
                    Some(owner.clone()),
                ),
                Instant::now(),
            ),
        );
        true
    }

    /// Record the new child on its creator, **then** pay off an end that arrived
    /// before the creation committed (audit-3 §3).
    ///
    /// The order is the point: a parked end deposits artifacts, steers the
    /// creator and publishes `ChildFinished`, and the creator has to be able to
    /// name the child before any of that happens.
    pub(super) fn publish_child_created_then_end(
        &self,
        creator_runtime: Option<&Arc<SessionRuntime>>,
        child: &str,
        display_name: &str,
        provider: &str,
        preset: &str,
        deferred: Option<DeferredChildEnd>,
    ) {
        if let Some(runtime) = creator_runtime {
            if !runtime.publish_child_created(child, display_name, provider, preset) {
                runtime.mark_journal_degraded();
            }
            // The record is in the creator's journal through the runtime above;
            // a runtime that is gone by now cannot be written to, and that is
            // stated rather than hidden.
        } else {
            // The creator's runtime is gone (its session closed while the child
            // was starting) and a journal record cannot be written without the
            // runtime's stream state — generation and sequence are its own. The
            // loss is stated rather than silent, and the S5B-09 commit check is
            // what keeps this path from being reachable by a creation that
            // should have been refused.
            eprintln!(
                "agent creation {child}: the creator's runtime is gone, so the creation record was not published"
            );
        }
        if let Some((session, runtime, owner)) = deferred {
            // The end that arrived before the link existed: report and release
            // it now, on this creation's thread (audit-2 §2).
            self.child_ended_with(child, session.as_ref(), runtime.as_deref(), owner.as_ref());
        }
    }

    pub(crate) fn child_ended_with(
        &self,
        child: &str,
        session: Option<&Session>,
        runtime: Option<&SessionRuntime>,
        owner: Option<&OwnerId>,
    ) {
        if let (Some(session), Some(runtime), Some(owner)) = (session, runtime, owner) {
            self.report_child_finish_with(child, session, runtime, owner);
        }
        self.release_agent_child(child);
    }

    /// The child is gone: give its slot back to its creator, and let a creator
    /// entry that is waiting for it go with it.
    pub(crate) fn release_agent_child(&self, child: &str) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(link) = table.children.remove(child) else {
            return;
        };
        let creator = link.creator;
        let started = link.started;
        if let Some(caps) = table.creators.get_mut(&creator) {
            if started {
                caps.live_children = caps.live_children.saturating_sub(1);
            } else if let Some((reservation, _)) = caps
                .in_flight
                .iter()
                .find(|(_, reserved)| reserved.as_str() == child)
                .map(|(id, reserved)| (*id, reserved.clone()))
            {
                // A child that ended before its spawn returned was never a
                // child: the reservation it still holds goes with it (audit
                // S5B-04), so an immediate exit leaks neither the slot nor the
                // window's count.
                caps.in_flight.remove(&reservation);
                caps.creations_in_window = caps.creations_in_window.saturating_sub(1);
                if caps.gate == CreationGate::Pending {
                    // The same rule as a released reservation (`S5B-02`): the
                    // question left with the child, so the next creation asks
                    // it again instead of being refused forever.
                    caps.gate = CreationGate::Closed;
                }
            }
        }
        let drop_creator = table
            .creators
            .get(&creator)
            .is_some_and(|caps| caps.creator_gone && caps.held() == 0);
        if drop_creator {
            table.creators.remove(&creator);
        }
    }

    /// The moment one child's `input_required` notice is owed, and the creator
    /// it is owed to. Answers `Some` exactly once per child.
    pub(super) fn claim_child_notice(&self, child: &str) -> Option<String> {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let link = table.children.get_mut(child)?;
        if !link.notice_owed || !link.notify {
            return None;
        }
        link.notice_owed = false;
        Some(link.creator.clone())
    }

    /// The same, for the finish report: `Some(creator, notify)` once per child,
    /// whatever path observes the end first.
    pub(super) fn claim_child_report(&self, child: &str) -> Option<(String, bool)> {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let link = table.children.get_mut(child)?;
        if !link.report_owed {
            return None;
        }
        link.report_owed = false;
        Some((link.creator.clone(), link.notify))
    }

    /// A live session's runtime, by id, when the session is this owner's.
    ///
    /// The owner check is the same one every registry read performs: a caller
    /// that learned an id may still not be its owner.
    pub(crate) fn live_runtime(
        &self,
        session_id: &str,
        owner: &OwnerId,
    ) -> Option<Arc<SessionRuntime>> {
        self.inner
            .lock()
            .ok()?
            .get(session_id)
            .filter(|entry| entry.owner().user == owner.user)
            .and_then(|entry| entry.as_peer_visible())
            .map(|live| Arc::clone(&live.runtime))
    }

    /// Raise the creation card on `creator` and wait for the person's answer
    /// (`S5` decision 4).
    ///
    /// False covers every way the answer was not an allow: a deny, the
    /// broker's timeout, a creator that is no longer running, a session with
    /// no broker to hold the card, and a card the broker refused because the
    /// device already had three pending. The caller refuses the creation in all
    /// of them, and the gate stays shut.
    pub(crate) fn ask_creation_card(
        &self,
        creator: &str,
        owner: &OwnerId,
        card: SessionEvent,
    ) -> bool {
        let Some(runtime) = self.live_runtime(creator, owner) else {
            return false;
        };
        let Some(broker) = runtime.permission_broker() else {
            return false;
        };
        broker.request_host_permission(card, &runtime) == permission_broker::HostDecision::Allow
    }

    /// A child's row and runtime, for the finish report.
    pub(super) fn child_view(
        &self,
        child: &str,
    ) -> Option<(Session, Arc<SessionRuntime>, OwnerId)> {
        let map = self.inner.lock().ok()?;
        let entry = map.get(child)?;
        let live = entry.as_peer_visible()?;
        Some((
            live_session_view(live),
            Arc::clone(&live.runtime),
            entry.owner().clone(),
        ))
    }

    /// Everything a child's transition owes its creator (`S5` §3): the
    /// `input_required` notice on its first parked card, and the finish report.
    ///
    /// Called on every transition the runtime notifies and on `close`, so it
    /// must be cheap when nothing is owed — both claims are one lock and one
    /// hash lookup, and they answer `None` for a session that is not an
    /// agent-created child at all, which is every session in a daemon nobody
    /// has commissioned from.
    pub(crate) fn report_child_events(&self, child: &str) {
        self.notify_child_input_required(child);
        self.report_child_finish(child);
    }

    /// One notice per child, on the first permission card it parks on.
    fn notify_child_input_required(&self, child: &str) {
        let Some((session, runtime, owner)) = self.child_view(child) else {
            return;
        };
        let parked = runtime
            .permission_broker()
            .is_some_and(|broker| broker.pending_len() > 0);
        if !parked {
            return;
        }
        let Some(creator) = self.claim_child_notice(child) else {
            return;
        };
        // A creator that is gone gets nothing: the child's card stays visible
        // on the child's own session, which is where a human answers it.
        let display_name = session
            .display_name
            .clone()
            .unwrap_or_else(|| session.title.clone());
        let envelope = agent_input_required_envelope(&session.id, &display_name, &session.origin);
        // The notice has no event beside it, so its delivery id is not needed:
        // a parked child is visible on its own session, and the text is the
        // whole message.
        let _ = self.deliver_to_creator(&creator, &owner, &envelope);
    }

    /// Read-only activity answer for one agent session: the derived headline,
    /// the hook's last state beside it (never merged), the idle age, and the
    /// bounded tail of the in-memory kind feed.
    ///
    /// Roster scope, deliberately: any session of the caller's own owner
    /// still in the live map — a commissioned child, a person-started
    /// session, or a recently ended one, which reads `unknown` with
    /// whatever feed survives in memory. A closed transcript is not found.
    /// Reading is not acting, and the roster already shows these rows. Metadata
    /// only — kinds, seqs, timestamps and the display name the roster
    /// already discloses; no payload text, no hook message. The broker
    /// resolves display names before calling, so this takes the exact id.
    pub(crate) fn agent_activity(
        &self,
        child: &str,
        owner: &OwnerId,
        limit: usize,
    ) -> Result<serde_json::Value, WireError> {
        let limit = limit.min(crate::agent_activity::ACTIVITY_MAX_LIMIT);
        let Some((session, runtime, child_owner)) = self.child_view(child) else {
            return Err(not_found());
        };
        if child_owner.user != owner.user {
            return Err(not_found());
        }
        let is_live = matches!(
            session.state,
            SessionState::Live { .. } | SessionState::Silent { .. }
        );
        let activity = crate::agent_activity::derive_activity(
            is_live,
            runtime.is_running_turn(),
            runtime.permission_pending(),
        );
        let activity_str = match activity {
            AgentActivityState::Idle => "idle",
            AgentActivityState::Working => "working",
            AgentActivityState::Blocked => "blocked",
            AgentActivityState::Unknown => "unknown",
        };
        let (hook_str, hook_seq) = match runtime.hook_activity() {
            Some((state, seq)) => (
                Some(match state {
                    AgentActivityState::Idle => "idle",
                    AgentActivityState::Working => "working",
                    AgentActivityState::Blocked => "blocked",
                    AgentActivityState::Unknown => "unknown",
                }),
                seq,
            ),
            None => (None, None),
        };
        let idle_ms = runtime
            .activity_idle_at(Instant::now())
            .map(|idle| idle.as_millis().try_into().unwrap_or(u64::MAX));
        let recent: Vec<serde_json::Value> = runtime
            .recent_activity(limit)
            .into_iter()
            .map(|mark| serde_json::json!({"seq": mark.seq, "kind": mark.kind, "tsMs": mark.ts_ms}))
            .collect();
        let display_name = session
            .display_name
            .clone()
            .unwrap_or_else(|| session.title.clone());
        Ok(serde_json::json!({
            "sessionId": session.id,
            "displayName": display_name,
            "activity": activity_str,
            "hookActivity": hook_str,
            "hookSeq": hook_seq,
            "idleMs": idle_ms,
            "lastSeq": runtime.last_seq(),
            "recent": recent,
        }))
    }

    /// One quiet notice per quiet spell. Returns whether a notice was sent.
    ///
    /// Notice, never action: the child's turn, disposition and brakes are
    /// untouched; only the creator gets one envelope. Blocked children are
    /// excluded (their permission card already has its own envelope), and a
    /// creator that set `notifyOnFinish: false` gets silence here too.
    pub(crate) fn notify_quiet_child(&self, child: &str, now: Instant) -> bool {
        let Some((session, runtime, owner)) = self.child_view(child) else {
            return false;
        };
        let is_live = matches!(
            session.state,
            SessionState::Live { .. } | SessionState::Silent { .. }
        );
        let Some(idle) = runtime.activity_idle_at(now) else {
            return false;
        };
        let turn = runtime.is_running_turn();
        let pending = runtime.permission_pending();
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(link) = table.children.get_mut(child) else {
            return false;
        };
        // Movement re-arms: a publish newer than the threshold means this
        // spell is over, whether or not a notice ever went out.
        if idle < crate::agent_activity::CHILD_QUIET_AFTER {
            link.quiet_notified = false;
            return false;
        }
        if !crate::agent_activity::quiet_due(is_live, turn, pending, idle, link.quiet_notified)
            || !link.notify
        {
            return false;
        }
        // The latch is set only after a delivery the creator actually got
        // (below): a lost notice must stay owed so the next sweep retries
        // the same spell instead of burning it.
        let creator = link.creator.clone();
        drop(table);
        let display_name = session
            .display_name
            .clone()
            .unwrap_or_else(|| session.title.clone());
        let idle_ms: u64 = idle.as_millis().try_into().unwrap_or(u64::MAX);
        let envelope = agent_quiet_envelope(&session.id, &display_name, idle_ms, &session.origin);
        let delivered = self
            .deliver_notice_to_creator(
                &creator,
                &owner,
                &envelope,
                crate::mcp_broker::ready_timeout(),
            )
            .is_ok();
        if delivered {
            if let Ok(mut table) = self.creations.lock() {
                if let Some(link) = table.children.get_mut(child) {
                    link.quiet_notified = true;
                }
            }
        }
        delivered
    }

    /// Sweep every commissioned child for quiet, sending at most one notice
    /// each. Driven once a minute by the server-owned thread; tests call it
    /// with injected instants.
    pub(crate) fn sweep_quiet_children(&self, now: Instant) -> usize {
        let children: Vec<String> = self
            .creations
            .lock()
            .map(|table| table.children.keys().cloned().collect())
            .unwrap_or_default();
        let mut sent = 0;
        for child in &children {
            if self.notify_quiet_child(child, now) {
                sent += 1;
            }
        }
        sent
    }

    /// The finish report: the deposit, the text message and the structured
    /// event, in that order (`S5` decisions 7 and 10).
    ///
    /// This is the *transition* caller, and a transition is not a finish: a
    /// child that parks on a card, or whose provider emits one malformed line,
    /// raises attention while it is still working. Reporting there would send
    /// the creator a `canceled` finish for a live child and spend the one
    /// report it is ever owed (`S5-01`), which the slice-5 e2e battery caught:
    /// the creator got "no message to deposit" half a second into a creation
    /// whose turn had not started. So the transition reports only for a child
    /// that has finished a turn ([`SessionRuntime::agent_stop_reason`], recorded
    /// before the attention raise in the same publish) or that is no longer
    /// live. Every path that *ends* a child reports through
    /// [`Self::child_ended_with`], unconditionally, because there the child is
    /// gone whatever its last turn said.
    pub(super) fn report_child_finish(&self, child: &str) {
        let Some((session, runtime, owner)) = self.child_view(child) else {
            return;
        };
        let ended = !matches!(
            session.state,
            SessionState::Live { .. } | SessionState::Silent { .. }
        );
        if !ended && runtime.agent_stop_reason().is_none() {
            return;
        }
        self.report_child_finish_with(child, &session, &runtime, &owner);
    }

    /// The same, with the child's row in hand.
    ///
    /// `close` takes the row out of the map before the runtime is torn down, so
    /// the caller that has the row passes it in rather than looking it up again.
    fn report_child_finish_with(
        &self,
        child: &str,
        session: &Session,
        runtime: &SessionRuntime,
        owner: &OwnerId,
    ) {
        let Some((creator, notify)) = self.claim_child_report(child) else {
            return;
        };
        // A caller that asked not to be told still gets its child's end
        // recorded on the child's own journal (the provider wrote it there);
        // what it asked to skip is this report.
        if !notify {
            return;
        }
        // The creator is gone: nothing is deposited and nothing is sent. The
        // child's journal still has its end; the creator's is closed.
        let Some(creator_runtime) = self.live_runtime(&creator, owner) else {
            return;
        };
        let (state, note) = child_finish_state(session, runtime);
        let snapshot = runtime.agent_message_snapshot();
        let (artifacts, note) = match snapshot.as_ref() {
            Some(snapshot) if !snapshot.text.is_empty() => {
                match self.deposit_child_message(&creator, owner, snapshot) {
                    Ok(artifact) => (vec![artifact], note),
                    // A deposit that fails never fails the report: the human
                    // still learns the child finished, and the note says the
                    // artifact is not there.
                    Err(reason) => (
                        Vec::new(),
                        Some(match note {
                            Some(note) => format!("{note} {reason}"),
                            None => reason,
                        }),
                    ),
                }
            }
            _ => (
                Vec::new(),
                Some(match note {
                    Some(note) => note,
                    None => "The child produced no message to deposit.".to_string(),
                }),
            ),
        };
        let display_name = session
            .display_name
            .clone()
            .unwrap_or_else(|| session.title.clone());
        let summary = summary_of(snapshot.as_ref().map(|snapshot| snapshot.text.as_str()));
        let envelope = bound_finish_envelope(agent_finished_envelope(
            &session.id,
            &display_name,
            state,
            &summary,
            &artifacts,
            note.as_deref(),
            &session.origin,
        ));
        // The delivery answers the id of the message it left on the creator's
        // transcript, and that id is what the event carries (audit S5-04). A
        // creator that is live but did not take the text is told so in the
        // note and gets no id at all: naming whatever message happened to be
        // last would point the app at somebody else's turn.
        let (message_id, note) = match self.deliver_to_creator(&creator, owner, &envelope) {
            Ok(Some(message_id)) => (Some(message_id), note),
            // Delivered, and the creator's provider kept no transcript record
            // for it: there is nothing to correlate to, and the text is there.
            Ok(None) => (None, note),
            Err(_) => (
                None,
                Some(match note {
                    Some(note) => format!("{note} finish report not delivered"),
                    None => "finish report not delivered".to_string(),
                }),
            ),
        };
        if !creator_runtime.publish_child_finished(
            message_id,
            &session.id,
            &display_name,
            state,
            note,
            artifacts,
        ) {
            creator_runtime.mark_journal_degraded();
        }
    }

    /// Hand one daemon-originated line to the creator through the slice-4
    /// steer-or-prompt path, **without** a sender brake slot.
    ///
    /// The exemption is deliberate and narrow: the brakes bound what an *agent*
    /// may spend on its peers, and this is the daemon's own report, raised by a
    /// child's end rather than by a caller. It is one delivery per finish.
    ///
    /// A peer's creator is never steered: the steer's refusal fallback is an
    /// interrupt, and interrupting a turn is `SessionInterrupt`'s act, which no
    /// capability opens to a peer (S4-01). A peer's report is a plain prompt.
    ///
    /// The answer is the transcript id the delivered text got (`S5-04`): the
    /// caller correlates an event to the message that is actually there, rather
    /// than reading a "last message" that may belong to somebody else. `None`
    /// means the text was delivered but left no transcript record.
    pub(super) fn deliver_to_creator(
        &self,
        creator: &str,
        owner: &OwnerId,
        text: &str,
    ) -> Result<Option<String>, WireError> {
        let local = self.creator_is_local(creator);
        // A refused steer must not take the report with it (audit S5B-06): the
        // steer is the preferred shape (it lands in the creator's turn instead
        // of queueing behind it), and when it is refused the same envelope goes
        // out once as a plain prompt. Only if that fails too does the caller
        // see an Err.
        let steer = self.send_to_creator(
            creator,
            owner,
            text,
            true,
            crate::mcp_broker::ready_timeout(),
        );
        if steer.is_ok() || !local {
            return steer;
        }
        self.send_to_creator(
            creator,
            owner,
            text,
            false,
            crate::mcp_broker::ready_timeout(),
        )
    }

    /// One daemon notice a creator is owed without urgency: it queues behind
    /// the creator's running turn as a plain prompt and can never steer. A
    /// separate function rather than a flag, so the urgent steer-or-prompt
    /// path above keeps its shape and no routine notice can pass the wrong
    /// boolean and interrupt a turn it only meant to inform.
    ///
    /// `mcp_timeout` is the caller's own budget for a creator whose broker
    /// has not come up yet: the idle sweep passes zero, because that thread
    /// owes every other child its cadence and a creator that is not ready
    /// *now* must not hold it (`session_idle_close.rs`).
    pub(super) fn deliver_notice_to_creator(
        &self,
        creator: &str,
        owner: &OwnerId,
        text: &str,
        mcp_timeout: Duration,
    ) -> Result<Option<String>, WireError> {
        self.send_to_creator(creator, owner, text, false, mcp_timeout)
    }

    fn send_to_creator(
        &self,
        creator: &str,
        owner: &OwnerId,
        text: &str,
        steer: bool,
        mcp_timeout: Duration,
    ) -> Result<Option<String>, WireError> {
        let internal_conn = ConnHandle::with_peer(0, None);
        self.send_with_subscription_timeout(&SendRequest {
            session_id: creator,
            subscription_id: 0,
            text,
            attachments: &[],
            // The daemon's own report carries no attachment: an agent message
            // may name a stored file, a `<devboule-system>` line may not.
            attachment_references: &[],
            owner,
            conn: &internal_conn,
            mcp_timeout,
            active_turn_behavior: steer.then_some(ActiveTurnBehavior::Steer),
            require_attachment: false,
            interrupt_on_steer_refusal: steer,
            message_slot: None,
            // No preset preamble: the daemon's own report is not a creation's
            // prompt, and a child that was created already had its first one.
            preset_preamble: None,
            spawn_prompt: None,
            // The daemon's own report about a child agent, never the person's
            // words: rendered as non-human alongside agent echoes.
            author: UserMessageAuthor::Agent,
            message_kind: UserMessageKind::SystemNotice,
        })
        .map(|outcome| outcome.message_id)
    }

    /// Whether the stored row for `session_id` says the person at this machine
    /// asked for it. Read from the row, never from a connection.
    fn creator_is_local(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|map| map.get(session_id).map(|entry| entry.to_session()))
            .is_some_and(|session| session.origin.is_local())
    }

    pub(crate) fn live_agent_entries(
        &self,
        owner: &OwnerId,
    ) -> Result<Vec<LiveAgentEntry>, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let mut sessions = map
            .values()
            .filter_map(|entry| {
                let live = entry.as_peer_visible()?;
                if live.owner.user != owner.user
                    || !crate::mcp_broker::hosts_mcp(&live.metadata.kind)
                {
                    return None;
                }
                let session = live_session_view(live);
                matches!(
                    session.state,
                    SessionState::Live { .. } | SessionState::Silent { .. }
                )
                .then(|| LiveAgentEntry {
                    session,
                    runtime: Arc::clone(&live.runtime),
                })
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.session.id.cmp(&right.session.id));
        Ok(sessions)
    }
}
